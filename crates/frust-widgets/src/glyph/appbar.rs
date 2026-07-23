//! Glyph top AppBar — core (task 04, glyph-refinements): the compact
//! terminal-native top bar per `research/glyph-appbar.html` sections 01/03/05,
//! retrieved 2026-07-22.
//!
//! # Anatomy (§01)
//!
//! A `52px` content bar of three zones — **leading** (a back arrow / brand mark
//! / nothing, an app-supplied [`AnyView`]), **title** (a title string plus an
//! optional 10px subtitle row, ellipsized), and **trailing** (0–2 app-supplied
//! action [`AnyView`]s). Icon hit targets are `40px`, `6px` horizontal edge
//! padding, `2px` slot gaps (the HTML's `.appbar`/`.ab-icon` rules).
//!
//! # Top-inset consumption (the Flutter parity point)
//!
//! Layout adds [`WindowInsets::padding().top`](frust_core::WindowInsets::padding)
//! to its own height and offsets its content **below** the inset; the background
//! paints the full extended height, so the bar surface runs under the status
//! bar. Content beneath a Glyph AppBar therefore never needs a top `SafeArea`
//! edge — exactly Flutter's `AppBar` behavior.
//!
//! # Elevation (§01)
//!
//! [`AppBarView::elevated`] is an app-fed flag (the app toggles it off its own
//! `on_scroll`, the same way the HTML flips `.elevated` at `y > 4`): a
//! transparent bottom border animates to `outline` + a drop shadow over
//! `~250ms`, collapsing instantly under `reduce_motion`.
//!
//! # Title crossfade (§03)
//!
//! When the title string changes across rebuilds, the bar stages the old and
//! new runs with a directional shift (`±14px` x-offset + fade over `~220ms`),
//! driven from `PaintCtx::frame_time` (the dialog/toast staging precedent — the
//! bar shapes its own runs like [`super::navbar`] rather than nesting a
//! `PatternSwitcher`). Direction is forward (new slides in from the right) by
//! default; [`AppBarView::title_direction`] flips it to back-nav.
//!
//! # Selection mode (§05)
//!
//! [`AppBarView::selection`]`(Some(`[`SelectionBar`]`))` morphs the bar into a
//! selection face: the surface tints with a `primary_container`-family wash, the
//! leading slot becomes a widget-owned `×`-close (firing `on_close` on
//! up-inside), the title becomes `"N selected"`, and the trailing swaps to the
//! caller-supplied bulk-action views. The title↔count face crosses over
//! vertically (`±6px` y + fade); the trailing action set swaps structurally on
//! the rebuild that toggled selection (a full dual-face child crossfade is out
//! of scope — the animated cues are the tint, the title↔count morph, and the
//! leading↔close swap).
//!
//! # Colors (accent-role split)
//!
//! Every color resolves from [`Theme::scheme`] with a Glyph **dark** constant
//! fallback (the [`super::navbar`] `resolve_nav_colors` precedent) — no
//! `StatusPalette` here. The selection wash is `primary_container` (the bright
//! *fill* role) while the count/close ink is `primary` (the accent *ink* role);
//! conflating the two is the catalog's most common accent bug
//! (`docs/CODE_STANDARDS.md`'s Glyph accent-role split).
//!
//! # Controlled component
//!
//! All state — title, subtitle, elevation, selection, actions — flows in from
//! the view every rebuild; the widget never self-mutates app-visible state.

use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View,
    Widget,
};
use frust_text::{FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, Size};
use peniko::{Brush, Color};

use crate::Timing;
use crate::nav::transition::{TransitionDriver, make_driver};

// ---- Metrics (`glyph-appbar.html` §01, retrieved 2026-07-22) ---------------

/// Compact content-bar height, logical px (`.appbar{height:52px}`). The top
/// inset is added on top of this (see the [module docs](self)).
const BAR_HEIGHT: f64 = 52.0;
/// Horizontal edge padding, logical px (`.appbar{padding:0 6px}`).
const PAD_X: f64 = 6.0;
/// Gap between adjacent slots, logical px (`.appbar{gap:2px}`).
const SLOT_GAP: f64 = 2.0;
/// Icon hit-target size, logical px (`.ab-icon{width:40px;height:40px}`). Also
/// the widget-owned selection close button's box.
const ICON_SIZE: f64 = 40.0;
/// Title font size, logical px (`.ab-title{font-size:13px}`).
const TITLE_SIZE: f32 = 13.0;
/// Subtitle font size, logical px (`.ab-subtitle{font-size:10px}`).
const SUBTITLE_SIZE: f32 = 10.0;
/// Selection count font size, logical px (`.sel-count{font-size:13px}`).
const COUNT_SIZE: f32 = 13.0;
/// Vertical gap between the title and subtitle rows, logical px (this task's
/// own choice — a tight 1px separation matching the HTML's stacked rows).
const SUBTITLE_GAP: f64 = 1.0;

/// Title-crossfade x-shift, logical px (`.bn-title-a{translateX(-14px)}`, §03).
const TITLE_SHIFT: f64 = 14.0;
/// Selection-morph y-shift, logical px (`.sel-normal{translateY(-6px)}`, §05).
const SELECTION_SHIFT: f64 = 6.0;
/// Selection wash alpha over the surface (`--amber-faint` = accent at 12%; here
/// sourced from the `primary_container` fill role per the accent-role split).
const SELECTION_WASH_ALPHA: f32 = 0.12;

/// Elevation animation duration (`.appbar{transition:...box-shadow .25s}`, §01).
const ELEVATION_DURATION: Duration = Duration::from_millis(250);
/// Selection-morph animation duration (`durations.base` — the same 220ms the
/// toast/dialog enter uses).
const SELECTION_DURATION: Duration = Duration::from_millis(220);
/// Title-crossfade duration (§03's "small directional shift", `durations.base`).
const TITLE_DURATION: Duration = Duration::from_millis(220);
/// Title-crossfade easing (the HTML's `--ease-out` cubic).
const TITLE_CURVE: Curve = Curve::Cubic(0.16, 1.0, 0.3, 1.0);
/// `reduce_motion`'s collapsed crossfade duration (mirrors the toast/dialog
/// constant of the same name).
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(120);

/// Elevated drop-shadow recipe (`.appbar.elevated{box-shadow:0 6px 16px
/// rgba(0,0,0,0.28)}`, §01).
const SHADOW_Y: f64 = 6.0;
const SHADOW_BLUR: f64 = 16.0;
const SHADOW_ALPHA: f32 = 0.28;

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// Bar surface — `--bg-surface` / themed `surface_container`.
const FALLBACK_SURFACE: Color = Color::from_rgb8(0x16, 0x1a, 0x23);
/// Title ink — `--fg` / themed `on_surface`.
const FALLBACK_TITLE_INK: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);
/// Subtitle ink — `--fg-muted` / themed `on_surface_variant`.
const FALLBACK_SUBTITLE_INK: Color = Color::from_rgb8(0xa3, 0x9c, 0x88);
/// Elevated border — `--border-bright` / themed `outline`.
const FALLBACK_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Accent ink (count / close) — `--amber` / themed `primary`.
const FALLBACK_ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Selection wash fill — `--amber` fill / themed `primary_container`.
const FALLBACK_WASH: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Elevated shadow color base — black (themed `scheme.shadow`).
const FALLBACK_SHADOW: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// The resolved bar color set.
struct BarColors {
    surface: Color,
    title_ink: Color,
    subtitle_ink: Color,
    border: Color,
    accent: Color,
    wash: Color,
    shadow: Color,
}

/// Resolve every bar color from the theme, falling back to the literal Glyph
/// **dark** constants with no theme threaded (the [`super::navbar`]
/// `resolve_nav_colors` precedent).
fn resolve_colors(theme: Option<&Theme>) -> BarColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            BarColors {
                surface: scheme.surface_container,
                title_ink: scheme.on_surface,
                subtitle_ink: scheme.on_surface_variant,
                border: scheme.outline,
                accent: scheme.primary,
                wash: scheme.primary_container,
                shadow: scheme.shadow,
            }
        }
        None => BarColors {
            surface: FALLBACK_SURFACE,
            title_ink: FALLBACK_TITLE_INK,
            subtitle_ink: FALLBACK_SUBTITLE_INK,
            border: FALLBACK_BORDER,
            accent: FALLBACK_ACCENT,
            wash: FALLBACK_WASH,
            shadow: FALLBACK_SHADOW,
        },
    }
}

/// Replace `color`'s alpha channel with `alpha` (the per-module helper shape
/// used across `frust-widgets`).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The Glyph `body` (IBM Plex Mono) font stack every run in this bar shapes
/// against.
fn mono_family() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

fn title_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TITLE_SIZE, color)
    }
}

fn subtitle_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(SUBTITLE_SIZE, color)
    }
}

fn count_style(color: Color) -> TextStyle {
    TextStyle {
        family: mono_family(),
        weight: FontWeight::SEMI_BOLD,
        ..TextStyle::new(COUNT_SIZE, color)
    }
}

/// The `(enter, _)` [`Timing`] for the title crossfade, honoring
/// `reduce_motion` (a short linear crossfade) — mirrors the toast/dialog
/// `resolve_timings` shape.
fn title_timing(reduce_motion: bool) -> Timing {
    if reduce_motion {
        Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear)
    } else {
        Timing::Duration(TITLE_DURATION, TITLE_CURVE)
    }
}

/// Direction the title crossfade shifts on a title change (§03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TitleDirection {
    /// The new title slides in from the right (default, forward navigation).
    #[default]
    Forward,
    /// The new title slides in from the left (back navigation).
    Back,
}

/// A minimal retained shaped text run — see [`super::navbar`]'s `GlyphLabel`.
struct ShapedRun {
    content: String,
    layout: Option<TextLayout>,
    laid_style: Option<TextStyle>,
    laid_max_width: Option<f32>,
}

impl ShapedRun {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_style: None,
            laid_max_width: None,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_max_width == max_width
            && self.laid_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_style = Some(style.clone());
        self.laid_max_width = max_width;
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map(|l| l.size()).unwrap_or(Size::ZERO)
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A staged title crossfade: the outgoing run plus its progress driver.
struct TitleStage {
    old: ShapedRun,
    driver: Option<TransitionDriver>,
    /// The last-advanced `0.0..=1.0` progress (`0.0` = old fully shown).
    progress: f64,
}

/// A view-held selection callback (`Fn(&mut State)`).
type OnClose<State> = Rc<dyn Fn(&mut State)>;

/// The selection-mode configuration (§05): a count, a close callback, and the
/// bulk-action views that replace the normal trailing actions.
pub struct SelectionBar<State: 'static> {
    count: usize,
    on_close: OnClose<State>,
    actions: Vec<AnyView<State>>,
}

/// Enter selection mode with `count` selected items and an `on_close` callback
/// (fired when the `×`-close is tapped). Attach bulk actions with
/// [`SelectionBar::actions`].
pub fn selection_bar<State: 'static, F: Fn(&mut State) + 'static>(
    count: usize,
    on_close: F,
) -> SelectionBar<State> {
    SelectionBar {
        count,
        on_close: Rc::new(on_close),
        actions: Vec::new(),
    }
}

impl<State: 'static> SelectionBar<State> {
    /// Set the trailing bulk-action views (in reading order — the last sits
    /// closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }
}

/// The `"N selected"` count string for a selection face.
fn count_text(count: usize) -> String {
    format!("{count} selected")
}

/// A declarative Glyph top AppBar. See the [module docs](self).
pub struct AppBarView<State: 'static> {
    title: String,
    subtitle: Option<String>,
    leading: Option<AnyView<State>>,
    actions: Vec<AnyView<State>>,
    elevated: bool,
    title_direction: TitleDirection,
    selection: Option<SelectionBar<State>>,
}

/// Create a Glyph AppBar titled `title`, with no leading slot, actions,
/// subtitle, elevation, or selection (attach them with the builder methods).
pub fn app_bar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    AppBarView {
        title: title.into(),
        subtitle: None,
        leading: None,
        actions: Vec::new(),
        elevated: false,
        title_direction: TitleDirection::Forward,
        selection: None,
    }
}

/// PascalCase alias for [`app_bar`], matching the widget-fn vocabulary.
#[allow(non_snake_case)]
pub fn AppBar<State: 'static>(title: impl Into<String>) -> AppBarView<State> {
    app_bar(title)
}

impl<State: 'static> AppBarView<State> {
    /// Set the optional subtitle row (10px, `on_surface_variant`), shown beneath
    /// the title in the normal (non-selection) face.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Set the leading slot (a back arrow, brand mark, or any app composition),
    /// erased as an [`AnyView`]. Its tint is the supplied view's responsibility.
    pub fn leading(mut self, leading: AnyView<State>) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Set the trailing action slots (0–2, in reading order — the last sits
    /// closest to the trailing edge).
    pub fn actions(mut self, actions: Vec<AnyView<State>>) -> Self {
        self.actions = actions;
        self
    }

    /// Set the scrolled-elevation flag (the app feeds this off its own
    /// `on_scroll`; see the [module docs](self)).
    pub fn elevated(mut self, elevated: bool) -> Self {
        self.elevated = elevated;
        self
    }

    /// Set the title-crossfade direction for the next title change (§03).
    pub fn title_direction(mut self, direction: TitleDirection) -> Self {
        self.title_direction = direction;
        self
    }

    /// Enter/leave selection mode (§05). `Some` morphs the bar into its
    /// selection face; `None` (the default) is the normal face.
    pub fn selection(mut self, selection: Option<SelectionBar<State>>) -> Self {
        self.selection = selection;
        self
    }
}

/// Collect the current face's interactive child views into one ordered slice —
/// the normal face's `leading? ++ actions`, or the selection face's bulk
/// actions (its close button is widget-owned, not a child view).
fn face_views<State: 'static>(view: &AppBarView<State>) -> Vec<&AnyView<State>> {
    match &view.selection {
        Some(sel) => sel.actions.iter().collect(),
        None => {
            let mut views = Vec::with_capacity(1 + view.actions.len());
            if let Some(leading) = &view.leading {
                views.push(leading);
            }
            views.extend(view.actions.iter());
            views
        }
    }
}

/// The retained widget for an [`AppBarView`]. See the [module docs](self).
pub struct AppBarWidget {
    title: ShapedRun,
    title_text: String,
    subtitle: Option<ShapedRun>,
    subtitle_text: Option<String>,
    count: ShapedRun,
    count_value: usize,
    /// The current face's interactive children (normal: `leading? ++ actions`;
    /// selection: bulk actions). Reconciled per-face — a face swap rebuilds it.
    interactive: Vec<ChildPod>,
    /// Whether `interactive[0]` is the normal-face leading slot.
    has_leading: bool,
    selection_present: bool,
    on_close: Option<crate::ErasedCallback>,
    title_direction: TitleDirection,
    elevated_target: bool,
    // --- animation state (advanced from `PaintCtx::frame_time`) ---
    elev_progress: f64,
    sel_progress: f64,
    title_stage: Option<TitleStage>,
    last_time: Option<FrameTime>,
    // --- layout-cached geometry (local coordinates) ---
    top_inset: f64,
    total_size: Size,
    title_pos: Point,
    subtitle_pos: Point,
    count_pos: Point,
    /// The selection close button's hit box, in widget-local coordinates.
    close_rect: Rect,
    close_pressed: bool,
    close_captured: bool,
}

impl<State: 'static> View<State> for AppBarView<State> {
    type Element = AppBarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AppBarWidget {
        let interactive = face_views(self)
            .iter()
            .map(|v| crate::build_child(v, ctx))
            .collect();
        let selection_present = self.selection.is_some();
        AppBarWidget {
            title: ShapedRun::new(self.title.clone()),
            title_text: self.title.clone(),
            subtitle: self.subtitle.as_ref().map(ShapedRun::new),
            subtitle_text: self.subtitle.clone(),
            count: ShapedRun::new(count_text(self.selection.as_ref().map_or(0, |s| s.count))),
            count_value: self.selection.as_ref().map_or(0, |s| s.count),
            interactive,
            has_leading: !selection_present && self.leading.is_some(),
            selection_present,
            on_close: self
                .selection
                .as_ref()
                .map(|s| crate::erase_callback(&s.on_close)),
            title_direction: self.title_direction,
            elevated_target: self.elevated,
            elev_progress: if self.elevated { 1.0 } else { 0.0 },
            sel_progress: if selection_present { 1.0 } else { 0.0 },
            title_stage: None,
            last_time: None,
            top_inset: 0.0,
            total_size: Size::ZERO,
            title_pos: Point::ZERO,
            subtitle_pos: Point::ZERO,
            count_pos: Point::ZERO,
            close_rect: Rect::ZERO,
            close_pressed: false,
            close_captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AppBarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.title_direction = self.title_direction;

        // Title change → stage a directional crossfade (§03).
        if prev.title != self.title {
            element.title_stage = Some(TitleStage {
                old: ShapedRun::new(prev.title.clone()),
                driver: None,
                progress: 0.0,
            });
            element.title = ShapedRun::new(self.title.clone());
            element.title_text = self.title.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Subtitle reconcile.
        if prev.subtitle != self.subtitle {
            element.subtitle = self.subtitle.as_ref().map(ShapedRun::new);
            element.subtitle_text = self.subtitle.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Selection count reconcile.
        let count = self.selection.as_ref().map_or(0, |s| s.count);
        if prev.selection.as_ref().map_or(0, |s| s.count) != count {
            element.count = ShapedRun::new(count_text(count));
            element.count_value = count;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Elevation target.
        if prev.elevated != self.elevated {
            element.elevated_target = self.elevated;
            flags |= ChangeFlags::PAINT;
        }

        // Face reconcile. A selection-presence toggle is a full face swap: tear
        // down the old face's children and build the new face's set (the two
        // faces are disjoint view sets, so positional reconciliation would
        // mismatch). Clear any in-flight close-button capture at the same time.
        let selection_present = self.selection.is_some();
        if prev.selection.is_some() != selection_present {
            for (view, pod) in face_views(prev)
                .into_iter()
                .zip(element.interactive.iter_mut())
            {
                crate::teardown_child(view, pod, ctx);
            }
            element.interactive = face_views(self)
                .iter()
                .map(|v| crate::build_child(v, ctx))
                .collect();
            element.selection_present = selection_present;
            element.has_leading = !selection_present && self.leading.is_some();
            element.close_pressed = false;
            element.close_captured = false;
            element.on_close = self
                .selection
                .as_ref()
                .map(|s| crate::erase_callback(&s.on_close));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // Same face: reconcile children in place, refresh the close adapter.
            let prev_views = face_views(prev);
            let next_views = face_views(self);
            flags |= crate::rebuild_children(
                &prev_views,
                &next_views,
                &mut element.interactive,
                ctx,
                |v: &&AnyView<State>| *v,
                |_| None,
            );
            if !selection_present && element.has_leading != self.leading.is_some() {
                element.has_leading = self.leading.is_some();
                flags |= ChangeFlags::LAYOUT;
            }
            element.on_close = self
                .selection
                .as_ref()
                .map(|s| crate::erase_callback(&s.on_close));
        }
        flags
    }

    fn teardown(&self, element: &mut AppBarWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in face_views(self)
            .into_iter()
            .zip(element.interactive.iter_mut())
        {
            crate::teardown_child(view, pod, ctx);
        }
    }
}

/// Move `cur` linearly toward `target` by `dt / dur`, returning whether it is
/// still short of the target afterward. The elevation/selection morphs use this
/// (a reversible retarget the bounded `TransitionDriver` can't express); the
/// title crossfade uses a real driver instead.
fn step(cur: &mut f64, target: f64, dt: Duration, dur: Duration) -> bool {
    if (*cur - target).abs() < 1e-6 {
        *cur = target;
        return false;
    }
    let delta = dt.as_secs_f64() / dur.as_secs_f64();
    if *cur < target {
        *cur = (*cur + delta).min(target);
    } else {
        *cur = (*cur - delta).max(target);
    }
    (*cur - target).abs() >= 1e-6
}

impl AppBarWidget {
    /// Advance every animation to frame time `now`, returning whether any is
    /// still in flight (the caller requests another frame). Factored out of
    /// `paint` so the timelines are drivable with synthetic [`FrameTime`]s in a
    /// unit test (the toast/dialog `advance` precedent).
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        let dt = self
            .last_time
            .map(|t| now.saturating_sub(t))
            .unwrap_or(Duration::ZERO);
        self.last_time = Some(now);

        let mut animating = false;
        let elev_target = if self.elevated_target { 1.0 } else { 0.0 };
        let sel_target = if self.selection_present { 1.0 } else { 0.0 };
        if reduce_motion {
            self.elev_progress = elev_target;
            self.sel_progress = sel_target;
        } else {
            animating |= step(&mut self.elev_progress, elev_target, dt, ELEVATION_DURATION);
            animating |= step(&mut self.sel_progress, sel_target, dt, SELECTION_DURATION);
        }

        if let Some(stage) = &mut self.title_stage {
            let adv = stage
                .driver
                .get_or_insert_with(|| make_driver(title_timing(reduce_motion)).0)
                .advance(now);
            stage.progress = adv.value.clamp(0.0, 1.0);
            if adv.done {
                self.title_stage = None;
            } else {
                animating = true;
            }
        }
        animating
    }

    /// The `(new_dx, old_dx)` title-crossfade x-offsets for progress `p` under
    /// the widget's [`TitleDirection`]: the new run slides in, the old slides
    /// out the opposite way.
    fn title_shift(&self, p: f64) -> (f64, f64) {
        let sign = match self.title_direction {
            TitleDirection::Forward => 1.0,
            TitleDirection::Back => -1.0,
        };
        let new_dx = sign * (1.0 - p) * TITLE_SHIFT;
        let old_dx = -sign * p * TITLE_SHIFT;
        (new_dx, old_dx)
    }
}

impl Widget for AppBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let colors = resolve_colors(Theme::from_layout_ctx(ctx));

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let top_inset = ctx.window_insets().padding().top;
        self.top_inset = top_inset;
        let content_top = top_inset;
        let icon_y = content_top + (BAR_HEIGHT - ICON_SIZE) / 2.0;
        let slot_bc = BoxConstraints::loose(Size::new(f64::INFINITY, BAR_HEIGHT));

        // Leading edge: the selection close button (owned) or the normal-face
        // leading child, whichever this face uses.
        let mut left = PAD_X;
        let action_start;
        if self.selection_present {
            self.close_rect =
                Rect::from_origin_size(Point::new(PAD_X, icon_y), Size::new(ICON_SIZE, ICON_SIZE));
            left = PAD_X + ICON_SIZE + SLOT_GAP;
            action_start = 0;
        } else {
            self.close_rect = Rect::ZERO;
            if self.has_leading {
                let size = self.interactive[0].layout_child(ctx, &slot_bc);
                self.interactive[0].set_origin(Point::new(
                    left,
                    content_top + (BAR_HEIGHT - size.height) / 2.0,
                ));
                left += size.width + SLOT_GAP;
                action_start = 1;
            } else {
                action_start = 0;
            }
        }

        // Trailing actions, laid out right-to-left so the last lands flush
        // against the trailing edge (reading order preserved).
        let mut right = width - PAD_X;
        for pod in self.interactive[action_start..].iter_mut().rev() {
            let size = pod.layout_child(ctx, &slot_bc);
            right -= size.width;
            pod.set_origin(Point::new(
                right,
                content_top + (BAR_HEIGHT - size.height) / 2.0,
            ));
            right -= SLOT_GAP;
        }

        // Title zone fills the middle; its runs are left-aligned and ellipsized
        // to the available width.
        let zone_x = left;
        let zone_w = (right - left).max(0.0);
        let title_size =
            self.title
                .layout(ctx, &title_style(colors.title_ink), Some(zone_w as f32));
        if let Some(stage) = &mut self.title_stage {
            stage
                .old
                .layout(ctx, &title_style(colors.title_ink), Some(zone_w as f32));
        }
        let subtitle_size = self.subtitle.as_mut().map(|s| {
            s.layout(
                ctx,
                &subtitle_style(colors.subtitle_ink),
                Some(zone_w as f32),
            )
        });
        self.count
            .layout(ctx, &count_style(colors.accent), Some(zone_w as f32));

        // Vertically center the title (+subtitle) stack inside the content band.
        let stack_h = title_size.height + subtitle_size.map_or(0.0, |s| SUBTITLE_GAP + s.height);
        let title_y = content_top + (BAR_HEIGHT - stack_h) / 2.0;
        self.title_pos = Point::new(zone_x, title_y);
        self.subtitle_pos = Point::new(zone_x, title_y + title_size.height + SUBTITLE_GAP);
        let count_h = self.count.size().height;
        self.count_pos = Point::new(zone_x, content_top + (BAR_HEIGHT - count_h) / 2.0);

        let total = bc.constrain(Size::new(width, BAR_HEIGHT + top_inset));
        self.total_size = total;
        total
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce_motion = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let colors = resolve_colors(theme);
        let animating = self.advance(ctx.frame_time(), reduce_motion);

        let origin = ctx.origin();
        let size = ctx.size();

        // Elevated drop shadow, beneath the surface fill so the surface covers
        // the shadow's near edge.
        if self.elev_progress > 0.0 {
            scene.draw_shadow(
                Point::new(origin.x, origin.y + SHADOW_Y),
                size,
                0.0,
                SHADOW_BLUR,
                with_alpha(colors.shadow, SHADOW_ALPHA * self.elev_progress as f32),
            );
        }

        // Surface: fills the full extended height (running under the status
        // bar), plus the selection wash overlay.
        scene.fill_rect(origin, size, colors.surface);
        if self.sel_progress > 0.0 {
            scene.fill_rect(
                origin,
                size,
                with_alpha(colors.wash, SELECTION_WASH_ALPHA * self.sel_progress as f32),
            );
        }

        // Elevated bottom hairline border.
        if self.elev_progress > 0.0 {
            scene.fill_rect(
                Point::new(origin.x, origin.y + size.height - 1.0),
                Size::new(size.width, 1.0),
                with_alpha(colors.border, self.elev_progress as f32),
            );
        }

        // Interactive children (current face), faded by the morph progress.
        let child_alpha = if self.selection_present {
            self.sel_progress
        } else {
            1.0 - self.sel_progress
        } as f32;
        if child_alpha > 0.0 {
            let layered = child_alpha < 1.0;
            if layered {
                scene.push_layer(origin, size, child_alpha);
            }
            for pod in &mut self.interactive {
                pod.paint_child(ctx, scene);
            }
            if layered {
                scene.pop_layer();
            }
        }

        // Selection close button (owned), fading in with the morph.
        if self.sel_progress > 0.0 {
            self.paint_close(origin, colors.accent, self.sel_progress as f32, scene);
        }

        // Normal title-zone content (title + subtitle), fading/sliding out under
        // the selection morph.
        let normal_alpha = (1.0 - self.sel_progress) as f32;
        if normal_alpha > 0.0 {
            let dy = -SELECTION_SHIFT * self.sel_progress;
            scene.push_transform(Affine::translate((0.0, dy)));
            let layered = normal_alpha < 1.0;
            if layered {
                scene.push_layer(origin, size, normal_alpha);
            }
            self.paint_title(origin, scene);
            if let Some(subtitle) = &self.subtitle {
                subtitle.paint(
                    Point::new(
                        origin.x + self.subtitle_pos.x,
                        origin.y + self.subtitle_pos.y,
                    ),
                    scene,
                );
            }
            if layered {
                scene.pop_layer();
            }
            scene.pop_transform();
        }

        // Selection count content, fading/sliding in.
        if self.sel_progress > 0.0 {
            let dy = SELECTION_SHIFT * (1.0 - self.sel_progress);
            scene.push_transform(Affine::translate((0.0, dy)));
            let alpha = self.sel_progress as f32;
            let layered = alpha < 1.0;
            if layered {
                scene.push_layer(origin, size, alpha);
            }
            self.count.paint(
                Point::new(origin.x + self.count_pos.x, origin.y + self.count_pos.y),
                scene,
            );
            if layered {
                scene.pop_layer();
            }
            scene.pop_transform();
        }

        if animating {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The widget-owned selection close button takes precedence over the
        // child action pods (it occupies the leading slot in selection mode).
        if self.selection_present
            && let InputEvent::Pointer(p) = event
        {
            if self.close_captured {
                match p.phase {
                    PointerPhase::Move => {
                        self.close_pressed = self.close_rect.contains(p.position);
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Up => {
                        if self.close_rect.contains(p.position)
                            && let Some(on_close) = &mut self.on_close
                        {
                            on_close(ctx);
                        }
                        self.close_pressed = false;
                        self.close_captured = false;
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Cancel => {
                        // Cancel never touches state — clear the flags only.
                        self.close_pressed = false;
                        self.close_captured = false;
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    PointerPhase::Down => {}
                }
            } else if p.phase == PointerPhase::Down && self.close_rect.contains(p.position) {
                self.close_pressed = true;
                self.close_captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                return EventResult::Handled;
            }
        }
        crate::route_event(&mut self.interactive, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = if self.selection_present {
            count_text(self.count_value)
        } else {
            self.title_text.clone()
        };
        let selection_present = self.selection_present;
        let has_leading = self.has_leading;
        let interactive = &self.interactive;
        ctx.push_container(
            Role::TitleBar,
            |node| node.set_label(label.as_str()),
            |ctx| {
                if selection_present {
                    // Leading close button (owned) first, then bulk actions.
                    ctx.push_node(Role::Button, |node| node.set_label("Cancel selection"));
                    for pod in interactive {
                        pod.semantics_child(ctx);
                    }
                } else {
                    if has_leading {
                        interactive[0].semantics_child(ctx);
                    }
                    for pod in &interactive[if has_leading { 1 } else { 0 }..] {
                        pod.semantics_child(ctx);
                    }
                }
            },
        );
    }
}

impl AppBarWidget {
    /// Paint the title zone's normal-face content: either the resting title run
    /// or the staged old/new crossfade pair (§03).
    fn paint_title(&mut self, origin: Point, scene: &mut dyn PaintScene) {
        let base = Point::new(origin.x + self.title_pos.x, origin.y + self.title_pos.y);
        match &self.title_stage {
            Some(stage) => {
                let p = stage.progress;
                let (new_dx, old_dx) = self.title_shift(p);
                let full = self.total_size;
                // Old run fades/slides out.
                scene.push_transform(Affine::translate((old_dx, 0.0)));
                scene.push_layer(origin, full, (1.0 - p) as f32);
                stage.old.paint(base, scene);
                scene.pop_layer();
                scene.pop_transform();
                // New (resting) run fades/slides in.
                scene.push_transform(Affine::translate((new_dx, 0.0)));
                scene.push_layer(origin, full, p as f32);
                self.title.paint(base, scene);
                scene.pop_layer();
                scene.pop_transform();
            }
            None => self.title.paint(base, scene),
        }
    }

    /// Paint the widget-owned selection `×`-close button as two diagonal strokes
    /// centered in [`close_rect`](Self::close_rect), tinted `accent` at `alpha`
    /// (platform-deterministic — no font-fallback dependency for the glyph).
    fn paint_close(&self, origin: Point, accent: Color, alpha: f32, scene: &mut dyn PaintScene) {
        // A 12px arm span centered in the 40px box.
        const ARM: f64 = 6.0;
        const STROKE_W: f64 = 1.5;
        let cx = origin.x + self.close_rect.center().x;
        let cy = origin.y + self.close_rect.center().y;
        let color = with_alpha(accent, alpha);
        let mut a = kurbo::BezPath::new();
        a.move_to((cx - ARM, cy - ARM));
        a.line_to((cx + ARM, cy + ARM));
        let mut b = kurbo::BezPath::new();
        b.move_to((cx + ARM, cy - ARM));
        b.line_to((cx - ARM, cy + ARM));
        scene.stroke_path(Point::ZERO, &a, STROKE_W, &Brush::Solid(color));
        scene.stroke_path(Point::ZERO, &b, STROKE_W, &Brush::Solid(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf_any;
    use frust_core::{
        BuildCtx, PointerButton, PointerEvent, RenderRoot, WindowEdgeInsets, WindowInsets,
    };
    use frust_text::TextContext;
    use std::any::Any;
    use std::cell::Cell;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn build(view: &AppBarView<()>) -> AppBarWidget {
        let mut counter = 0u64;
        View::<()>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn rebuild(prev: &AppBarView<()>, next: &AppBarView<()>, w: &mut AppBarWidget) -> ChangeFlags {
        let mut counter = 0u64;
        View::<()>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter))
    }

    /// A layout pass with a real `TextContext` (the title/subtitle/count are
    /// shaped runs, which panic without one) and an optional theme.
    fn layout(w: &mut AppBarWidget, size: Size, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        shadows: usize,
        strokes: usize,
        glyph_runs: usize,
        transforms: usize,
        transform_pops: usize,
        layers: usize,
        layer_pops: usize,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {
            self.shadows += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, _brush: &Brush) {
            self.strokes += 1;
        }
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {
            self.glyph_runs += 1;
        }
        fn push_transform(&mut self, _t: Affine) {
            self.transforms += 1;
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
        fn push_layer(&mut self, _o: Point, _s: Size, _a: f32) {
            self.layers += 1;
        }
        fn pop_layer(&mut self) {
            self.layer_pops += 1;
        }
    }

    fn paint(w: &mut AppBarWidget, now: FrameTime, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let size = w.total_size;
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        // The frame time is threaded via `RenderRoot::paint`; a bare `PaintCtx`
        // can't be seeded at an arbitrary time, so drive `advance` directly.
        let reduce = theme.map(|t| t.motion.reduce_motion).unwrap_or(false);
        let animating = w.advance(now, reduce);
        w.paint(&mut pctx, &mut rec);
        (rec, animating)
    }

    // -- Acceptance 1: top-inset consumption (Flutter parity) ---------------

    #[test]
    fn layout_consumes_top_inset_and_offsets_content_below_it() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("Home").leading(leaf_any(40.0, 40.0))
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.set_insets(WindowInsets::new(
            WindowEdgeInsets::new(0.0, 24.0, 0.0, 0.0),
            WindowEdgeInsets::ZERO,
        ));
        let mut tcx = TextContext::new();
        let size = root.layout_with_text(Size::new(400.0, 200.0), &mut tcx as &mut dyn Any);
        // 52px bar + 24px top inset = 76px.
        assert_eq!(size.height, BAR_HEIGHT + 24.0);

        let id = root.root_id().expect("root built");
        let w = (root.tree().pod(id).expect("pod").widget() as &dyn Any)
            .downcast_ref::<AppBarWidget>()
            .expect("root is an AppBarWidget");
        assert_eq!(w.top_inset, 24.0);
        // The leading child and the title both sit below the 24px inset.
        assert!(
            w.interactive[0].origin().y >= 24.0,
            "leading below the inset"
        );
        assert!(w.title_pos.y >= 24.0, "title below the inset");
    }

    // -- Acceptance 2: title crossfade staging ------------------------------

    #[test]
    fn title_change_stages_a_crossfade_and_requests_frames_while_animating() {
        let prev = app_bar("Sessions");
        let mut w = build(&prev);
        layout(&mut w, Size::new(360.0, 100.0), None);

        let next = app_bar("Settings");
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(w.title_stage.is_some(), "a title change stages a crossfade");
        assert_eq!(w.title_text, "Settings");
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seed the driver clock: still staging, another frame requested.
        let (_, animating0) = paint(&mut w, ft_ms(0.0), None);
        assert!(animating0);
        assert!(w.title_stage.is_some());

        // Past the 220ms crossfade: staging clears.
        let (_, animating1) = paint(&mut w, ft_ms(400.0), None);
        assert!(!animating1);
        assert!(w.title_stage.is_none(), "the crossfade settled");
    }

    #[test]
    fn title_direction_flips_the_crossfade_shift_sign() {
        let mut fwd = build(&app_bar("A"));
        fwd.title_direction = TitleDirection::Forward;
        let (fwd_new, fwd_old) = fwd.title_shift(0.0);
        // Forward: the new run enters from the right (+), old exits left (0 at p=0).
        assert!(fwd_new > 0.0);
        assert!(fwd_old.abs() < 1e-9);

        let mut back = build(&app_bar("A"));
        back.title_direction = TitleDirection::Back;
        let (back_new, _) = back.title_shift(0.0);
        assert!(back_new < 0.0, "back-nav enters from the left");
    }

    // -- Acceptance 3: selection mode morph + close ------------------------

    #[test]
    fn selection_swaps_the_face_and_close_fires_on_up_inside_only() {
        let prev: AppBarView<()> = app_bar("Tokens").actions(vec![leaf_any(24.0, 24.0)]);
        let mut w = build(&prev);
        assert!(!w.selection_present);
        assert_eq!(w.interactive.len(), 1, "normal face: one action");

        // The close callback bumps a captured counter (State stays `()`), so the
        // controlled-component contract is visible without a typed state.
        let closes = Rc::new(Cell::new(0u32));
        let c = closes.clone();
        let next: AppBarView<()> = app_bar("Tokens").selection(Some(
            selection_bar(3, move |_: &mut ()| c.set(c.get() + 1))
                .actions(vec![leaf_any(24.0, 24.0), leaf_any(24.0, 24.0)]),
        ));
        let flags = rebuild(&prev, &next, &mut w);
        assert!(flags.needs_layout());
        assert!(w.selection_present, "selection face active");
        assert_eq!(w.count_value, 3);
        assert_eq!(w.interactive.len(), 2, "selection face: two bulk actions");

        // Lay out so the close_rect is populated.
        layout(&mut w, Size::new(360.0, 100.0), None);
        assert!(w.close_rect.width() > 0.0, "close button laid out");

        let (cx, cy) = (w.close_rect.center().x, w.close_rect.center().y);
        let ev = |phase, x, y| {
            InputEvent::Pointer(PointerEvent {
                phase,
                position: Point::new(x, y),
                button: PointerButton::Primary,
            })
        };

        // Down inside, up outside: no fire (up must land inside).
        {
            let mut dummy = ();
            let state: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(state, Point::ZERO, w.total_size);
            w.event(&mut ctx, &ev(PointerPhase::Down, cx, cy));
            w.event(&mut ctx, &ev(PointerPhase::Up, 300.0, 20.0));
        }
        assert_eq!(closes.get(), 0, "up outside the close box does not fire");

        // Down + up inside: fires once.
        {
            let mut dummy = ();
            let state: &mut dyn Any = &mut dummy;
            let mut ctx = EventCtx::new(state, Point::ZERO, w.total_size);
            w.event(&mut ctx, &ev(PointerPhase::Down, cx, cy));
            w.event(&mut ctx, &ev(PointerPhase::Up, cx, cy));
        }
        assert_eq!(closes.get(), 1, "up-inside fires on_close exactly once");
    }

    #[test]
    fn selection_tint_animates_in_and_count_reads_n_selected() {
        let prev = app_bar("Tokens");
        let mut w = build(&prev);
        let next = app_bar("Tokens").selection(Some(selection_bar(5, |_: &mut ()| {})));
        rebuild(&prev, &next, &mut w);
        assert_eq!(w.count.content, "5 selected");
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seeded start: no wash yet (progress 0). It rises across frames.
        paint(&mut w, ft_ms(0.0), None);
        assert!(w.sel_progress < 1.0);
        paint(&mut w, ft_ms(400.0), None);
        assert!(
            (w.sel_progress - 1.0).abs() < 1e-6,
            "morph settles fully in"
        );
    }

    // -- Acceptance 4: elevation animation + reduce_motion ------------------

    #[test]
    fn elevation_animates_border_and_shadow_over_time() {
        let prev = app_bar("Home");
        let mut w = build(&prev);
        assert_eq!(w.elev_progress, 0.0);
        let next = app_bar("Home").elevated(true);
        rebuild(&prev, &next, &mut w);
        assert!(w.elevated_target);
        layout(&mut w, Size::new(360.0, 100.0), None);

        // Seed the clock: progress still ~0, no shadow yet, animation in flight.
        let (rec0, animating0) = paint(&mut w, ft_ms(0.0), None);
        assert!(animating0);
        assert_eq!(rec0.shadows, 0, "no shadow at progress 0");

        // Past the 250ms elevation: fully elevated, shadow + border drawn.
        let (rec1, _) = paint(&mut w, ft_ms(500.0), None);
        assert!((w.elev_progress - 1.0).abs() < 1e-6);
        assert_eq!(rec1.shadows, 1, "elevated shadow drawn");
    }

    #[test]
    fn reduce_motion_collapses_the_elevation_animation() {
        let mut theme = Theme::glyph_baseline();
        theme.motion.reduce_motion = true;
        let prev = app_bar("Home");
        let mut w = build(&prev);
        let next = app_bar("Home").elevated(true);
        rebuild(&prev, &next, &mut w);
        layout(&mut w, Size::new(360.0, 100.0), Some(&theme));
        // One paint snaps straight to fully elevated under reduce_motion.
        let (rec, animating) = paint(&mut w, ft_ms(0.0), Some(&theme));
        assert!(!animating, "reduce_motion snaps, no follow-up frame");
        assert!((w.elev_progress - 1.0).abs() < 1e-6);
        assert_eq!(rec.shadows, 1);
    }

    // -- Themed color resolution -------------------------------------------

    #[test]
    fn glyph_dark_and_light_resolve_surface_and_accent() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);
        for theme in [&dark, &light] {
            let c = resolve_colors(Some(theme));
            assert_eq!(c.surface, theme.scheme().surface_container);
            assert_eq!(c.title_ink, theme.scheme().on_surface);
            assert_eq!(c.accent, theme.scheme().primary);
            assert_eq!(c.wash, theme.scheme().primary_container);
        }
        // Accent-role split: primary (ink) and primary_container (fill) differ
        // in Glyph light mode.
        let lc = resolve_colors(Some(&light));
        assert_ne!(lc.accent, lc.wash, "accent ink != container fill in light");
    }

    #[test]
    fn unthemed_paint_uses_fallback_surface() {
        let mut w = build(&app_bar("Home"));
        layout(&mut w, Size::new(300.0, 100.0), None);
        let (rec, _) = paint(&mut w, ft_ms(0.0), None);
        assert_eq!(
            rec.rects[0].2, FALLBACK_SURFACE,
            "surface fill is the fallback"
        );
    }

    // -- Acceptance 5: semantics -------------------------------------------

    #[test]
    fn semantics_is_a_titlebar_labelled_with_the_title_and_ordered_children() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("Inbox")
                .leading(leaf_any(40.0, 40.0))
                .actions(vec![leaf_any(24.0, 24.0)])
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar container node");
        assert_eq!(node.label(), Some("Inbox"));
        // The leading/action pods are forwarded via `semantics_child` in
        // leading→actions order (the test-fixture leaves emit no node of their
        // own, so only the container itself is asserted here).
    }

    #[test]
    fn selection_semantics_labels_the_count_and_lists_the_close_button() {
        fn logic(_: &mut ()) -> AppBarView<()> {
            app_bar("Tokens").selection(Some(
                selection_bar(2, |_: &mut ()| {}).actions(vec![leaf_any(24.0, 24.0)]),
            ))
        }
        let mut root: RenderRoot<(), AppBarView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TitleBar)
            .expect("a TitleBar container node");
        assert_eq!(node.label(), Some("2 selected"));
        // The owned close button contributes a Button node (the bulk-action
        // leaf emits none of its own).
        assert_eq!(node.children().len(), 1, "the close Button node");
        assert!(
            update.nodes.iter().any(|(_, n)| n.role() == Role::Button),
            "the close button contributes a Button node"
        );
    }
}
