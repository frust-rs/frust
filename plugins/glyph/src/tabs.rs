//! [`tabs`]/[`TabsView`]: a horizontal tab strip with one moving 2px underline
//! indicator that animates its position/width to the active tab
//! (the Glyph design system's `.tabs`/`.tab`/`.tab.active` rules,
//! retrieved 2026-07-21).
//!
//! # Controlled selection (never self-mutating)
//!
//! Like [`frust::Checkbox`]/[`frust::Slider`] and the M3
//! `crate::material::navigation_bar`, this is a **controlled** component:
//! [`TabsView`] reports the *requested* index through `on_select(index)` and
//! never mutates its own `selected`. The app feeds the confirmed index back in
//! on the next rebuild, and the indicator animates from its current position to
//! the newly-confirmed tab (see the animation note below).
//!
//! # The moving indicator (advance-during-paint)
//!
//! A single 2px underline slides between tabs on a selection change:
//! position **and** width interpolate over the Glyph `220ms` spatial curve
//! ([`INDICATOR_DURATION`]/[`INDICATOR_CURVE`], the exact
//! `MotionScheme::glyph()` `durations.base`/`easing.spatial` values — hardcoded
//! for the same reason `crate::material::navbar`'s spring token is: an
//! [`AnimationController`]'s duration/curve has no deferred, post-`build`
//! theme-resolution seam the way a themed *color* does). The overshooting
//! spatial bezier lets the indicator briefly slide past its target before
//! settling — the "spatial" motion character — and it lands *exactly* under
//! the active tab once the animation completes. The controller is advanced
//! during `paint` (see `docs/ARCHITECTURE.md`'s Frame pipeline) via
//! [`TabsWidget::advance_indicator`].
//!
//! # Underline + color both carry active state
//!
//! The source rule is that the active tab is *never* signalled by color alone:
//! the active label switches to the accent AND grows the underline. This widget
//! honours both (accent label + accent underline for the active tab; the muted
//! `fg-dim` for the rest).
//!
//! # Horizontal overflow panning
//!
//! When the tabs' total intrinsic width exceeds the available width (many
//! sections on a narrow display), the strip pans itself horizontally rather
//! than clipping the trailing tabs off-screen — no new public API, purely
//! automatic. Layout records the `overflow` (content − viewport) and clamps a
//! retained `scroll_x ∈ [0, overflow]`; a wider relayout re-clamps it. A
//! pointer drag past [`TOUCH_SLOP`] takes the gesture over as a pan (mirroring
//! `crate::ScrollWidget`'s takeover), while a sub-slop press-and-release
//! still selects the tapped tab; a horizontal scroll/wheel delta
//! ([`WHEEL_LINE_PX`]) pans on desktop. On a confirmed selection change the
//! newly-active tab is scrolled fully into view, matching the sliding-indicator
//! UX. When the content fits there is zero behavioral delta (`scroll_x` stays
//! `0`).
//!
//! # Why this widget doesn't nest `Text` children
//!
//! See [`crate::badge`]'s module docs — the per-tab dynamic color
//! (accent vs muted, chosen at layout time from the theme) isn't one of
//! [`frust::TextView`]'s four fixed `ThemeTextColor` roles, so
//! `TabsWidget` shapes and paints its own label glyph runs directly via
//! `frust_text`, mirroring `crate::text::TextWidget`'s shape.

use std::rc::Rc;

use frust::Theme;
use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, ScrollDelta, SemanticsCtx, View, Widget,
};
use frust::input::{TOUCH_SLOP, WHEEL_LINE_PX};
use frust::{AnimationController, Curve, FrameTime};
use kurbo::{Point, Rect, Size, Vec2};
use peniko::Color;
use std::time::Duration;

use crate::press::presses;

/// Horizontal padding per tab, logical px (`.tab{padding:10px 4px}` → 4px
/// horizontal).
const TAB_PAD_X: f64 = 4.0;
/// Vertical padding per tab, logical px (`.tab{padding:10px 4px}` → 10px
/// vertical).
const TAB_PAD_Y: f64 = 10.0;
/// Gap between consecutive tabs, logical px (`.tab{margin-right:20px}` — the
/// dominant inter-tab spacing; the `.tabs{gap:4px}` flex gap is folded into
/// this single documented value).
const TAB_GAP: f64 = 20.0;
/// Label font size, logical px (`.tab{font-size:12px}`).
const TAB_FONT_SIZE: f32 = 12.0;
/// Underline indicator thickness, logical px (`.tab{border-bottom:2px}`).
const INDICATOR_H: f64 = 2.0;
/// Strip bottom-border thickness, logical px (`.tabs{border-bottom:1px}`).
const STRIP_BORDER_H: f64 = 1.0;

/// The indicator slide duration — the exact `MotionScheme::glyph()`
/// `durations.base` (220ms). See the module docs for why this is hardcoded
/// rather than theme-resolved.
const INDICATOR_DURATION: Duration = Duration::from_millis(220);
/// The indicator slide easing — the exact `MotionScheme::glyph()`
/// `easing.spatial` (`cubic-bezier(0.34, 1.35, 0.64, 1.0)`, an overshooting
/// "spatial" curve). See the module docs.
const INDICATOR_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// The accent (amber) — `--amber` / themed `primary`.
const TAB_ACCENT: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// The inactive label tone — `--fg-dim` / themed `on_surface_variant` (see the
/// `fg-dim`/`fg-muted` simplification documented in [`crate::tag`]).
const TAB_INACTIVE: Color = Color::from_rgb8(0x6b, 0x65, 0x56);
/// The strip's bottom hairline — `--border` / themed `outline_variant`.
const TAB_STRIP_BORDER: Color = Color::from_rgb8(0x2a, 0x2d, 0x33);

/// Resolve `(accent, inactive, strip-border)` from the theme, falling back to
/// the literal Glyph **dark** constants above with no theme threaded.
fn resolve_tab_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.primary,
                scheme.on_surface_variant,
                scheme.outline_variant,
            )
        }
        None => (TAB_ACCENT, TAB_INACTIVE, TAB_STRIP_BORDER),
    }
}

/// A view-held, typed selection callback (erased on build/rebuild).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative tab strip. See the [module docs](self).
pub struct TabsView<State: 'static> {
    labels: Vec<String>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Create a tab strip over `labels`, with `selected` the current (app-confirmed)
/// index. Fires `on_select(state, index)` on a release inside a tab — a
/// **controlled** component (`selected` is never self-mutated; feed the
/// confirmed index back on the next rebuild).
pub fn tabs<State: 'static, F: Fn(&mut State, usize) + 'static>(
    labels: Vec<String>,
    selected: usize,
    on_select: F,
) -> TabsView<State> {
    TabsView {
        labels,
        selected,
        on_select: Rc::new(on_select),
    }
}

/// A minimal retained text run — see [`crate::badge`]'s `GlyphLabel` for
/// the full shape/rationale (duplicated per file, matching this crate's
/// per-file small-helper convention).
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
    laid_out_max_width: Option<f32>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
            laid_out_max_width: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle, max_width: Option<f32>) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, max_width);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        self.laid_out_max_width = max_width;
        size
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// One retained tab: its label plus its layout-computed local geometry.
struct TabEntry {
    label: GlyphLabel,
    text: String,
    /// Local x of the tab's left edge (filled at layout).
    x: f64,
    /// The tab's full width incl. padding (filled at layout).
    width: f64,
    /// The label's own measured size (filled at layout).
    label_size: Size,
}

/// The label's fixed style (family/weight/size are Glyph-authored constants;
/// only `color` varies).
fn tab_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(TAB_FONT_SIZE, color)
    }
}

/// Component-wise lerp of an `(x, width)` indicator geometry.
fn lerp_geom(from: (f64, f64), to: (f64, f64), t: f64) -> (f64, f64) {
    (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
}

/// The retained widget for a [`TabsView`].
pub struct TabsWidget {
    tabs: Vec<TabEntry>,
    selected: usize,
    on_select: frust::authoring::ErasedArgCallback<usize>,
    height: f64,

    /// The slide controller (idle between selection changes).
    indicator: AnimationController,
    /// The indicator geometry the current slide started from.
    anim_from: (f64, f64),
    /// The indicator geometry the current slide is heading to.
    anim_to: (f64, f64),
    /// The indicator geometry painted this frame.
    displayed: (f64, f64),
    /// `false` until the first layout seeds the indicator geometry.
    have_geometry: bool,
    /// Set by `rebuild` when the confirmed selection changed; consumed by the
    /// next `layout` to start a slide (geometry is only known post-layout).
    pending: bool,

    /// The pressed *visual* index; follows the cursor in/out while captured.
    pressed: Option<usize>,
    /// The captured tab index, armed on `Down`, cleared on `Up`/`Cancel`.
    captured: Option<usize>,

    /// Retained horizontal pan offset, always clamped to `[0, overflow]`.
    /// Content is painted shifted left by this amount and hit-tests add it back.
    scroll_x: f64,
    /// How much the tabs' intrinsic width exceeds the viewport (`content −
    /// viewport`, never negative); `0` when everything fits.
    overflow: f64,
    /// The available (constrained) width the strip was last laid out into —
    /// the window used to scroll a selected tab fully into view.
    viewport_w: f64,
    /// Whether a `Down` armed a gesture (a tab tap and/or a pan). Set on `Down`,
    /// cleared on `Up`/`Cancel`; the slop/pan math runs only while it is true so
    /// a bare hover `Move` is never mistaken for a drag.
    down_active: bool,
    /// Whether the armed gesture crossed [`TOUCH_SLOP`] and became a pan (which
    /// suppresses the tab tap on release).
    panning: bool,
    /// The `Down` position, the anchor the slop is measured from.
    down_start: Point,
    /// The previous pan sample, differenced to move `scroll_x` each `Move`.
    last_pan: Point,
}

impl<State: 'static> View<State> for TabsView<State> {
    type Element = TabsWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TabsWidget {
        let tabs = self
            .labels
            .iter()
            .map(|l| TabEntry {
                label: GlyphLabel::new(l.clone()),
                text: l.clone(),
                x: 0.0,
                width: 0.0,
                label_size: Size::ZERO,
            })
            .collect();
        TabsWidget {
            tabs,
            selected: self.selected,
            on_select: frust::authoring::erase_callback_arg(&self.on_select),
            height: 0.0,
            indicator: AnimationController::new(INDICATOR_DURATION).with_curve(INDICATOR_CURVE),
            anim_from: (0.0, 0.0),
            anim_to: (0.0, 0.0),
            displayed: (0.0, 0.0),
            have_geometry: false,
            pending: false,
            pressed: None,
            captured: None,
            scroll_x: 0.0,
            overflow: 0.0,
            viewport_w: 0.0,
            down_active: false,
            panning: false,
            down_start: Point::ZERO,
            last_pan: Point::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TabsWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable — always reinstall the adapter.
        element.on_select = frust::authoring::erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;

        // Reconcile labels positionally (a tab strip is a fixed small set).
        if prev.labels.len() != self.labels.len() {
            element.tabs = self
                .labels
                .iter()
                .map(|l| TabEntry {
                    label: GlyphLabel::new(l.clone()),
                    text: l.clone(),
                    x: 0.0,
                    width: 0.0,
                    label_size: Size::ZERO,
                })
                .collect();
            element.have_geometry = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, label) in element.tabs.iter_mut().zip(self.labels.iter()) {
                if &entry.text != label {
                    entry.label.set_content(label.clone());
                    entry.text = label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }

        if prev.selected != self.selected {
            element.selected = self.selected;
            element.pending = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl TabsWidget {
    /// The target `(x, width)` geometry of the currently-selected tab, or
    /// `(0, 0)` if there are no tabs / the index is out of range.
    fn target_geometry(&self) -> (f64, f64) {
        self.tabs
            .get(self.selected)
            .map(|t| (t.x, t.width))
            .unwrap_or((0.0, 0.0))
    }

    /// The tab index under a widget-local pointer position, if any. The current
    /// pan offset is added back so a hit test lands on the tab actually painted
    /// under the cursor (paint shifts content left by `scroll_x`).
    fn hit_index(&self, pos: Point) -> Option<usize> {
        let content = Point::new(pos.x + self.scroll_x, pos.y);
        self.tabs.iter().position(|t| {
            Rect::from_origin_size(Point::new(t.x, 0.0), Size::new(t.width, self.height))
                .contains(content)
        })
    }

    /// Set the pan offset, clamped to the current pannable range `[0, overflow]`.
    fn set_scroll_x(&mut self, value: f64) {
        self.scroll_x = value.clamp(0.0, self.overflow);
    }

    /// Pan the minimum amount so the currently-selected tab is fully visible
    /// within the viewport (a no-op when nothing overflows). Called from layout
    /// on a confirmed selection change so the active tab is never off-screen.
    fn scroll_selected_into_view(&mut self) {
        if self.overflow <= 0.0 {
            return;
        }
        let Some(tab) = self.tabs.get(self.selected) else {
            return;
        };
        let left = tab.x;
        let right = tab.x + tab.width;
        if left < self.scroll_x {
            self.set_scroll_x(left);
        } else if right > self.scroll_x + self.viewport_w {
            self.set_scroll_x(right - self.viewport_w);
        }
    }

    /// Advance the slide to frame time `now`, updating [`Self::displayed`] and
    /// returning whether the indicator is still animating (in which case the
    /// caller must request another frame). Factored out of `paint` so the
    /// animation is drivable in a unit test — `PaintCtx::set_frame_time` is
    /// `pub(crate)` to `frust-core`, so a widget test can't thread synthetic
    /// frame times through `paint` (mirrors `crate::cupertino::tabbar`'s
    /// `drive`).
    fn advance_indicator(&mut self, now: FrameTime) -> bool {
        if self.indicator.is_animating() {
            let animating = self.indicator.advance(now);
            self.displayed = lerp_geom(self.anim_from, self.anim_to, self.indicator.value());
            animating
        } else {
            self.displayed = self.anim_to;
            false
        }
    }
}

impl Widget for TabsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (accent, inactive, _) = resolve_tab_colors(theme);

        let mut x = 0.0;
        let mut content_height = 0.0_f64;
        for (i, entry) in self.tabs.iter_mut().enumerate() {
            let color = if i == self.selected { accent } else { inactive };
            let style = tab_style(color);
            let label_size = entry.label.layout(ctx, &style, None);
            entry.label_size = label_size;
            entry.x = x;
            entry.width = label_size.width + TAB_PAD_X * 2.0;
            content_height = content_height.max(label_size.height);
            x += entry.width + TAB_GAP;
        }
        let total_width = if self.tabs.is_empty() {
            0.0
        } else {
            x - TAB_GAP
        };
        let height = content_height + TAB_PAD_Y * 2.0;
        self.height = height;

        // Overflow panning: measure how far the intrinsic content exceeds the
        // available width, re-clamp the retained pan against it (a wider window
        // shrinks the overflow), and bring the selected tab into view on a
        // confirmed selection change.
        let viewport_w = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            total_width
        };
        self.viewport_w = viewport_w;
        self.overflow = (total_width - viewport_w).max(0.0);
        self.set_scroll_x(self.scroll_x);
        if self.pending {
            self.scroll_selected_into_view();
        }

        let target = self.target_geometry();
        if !self.have_geometry {
            self.displayed = target;
            self.anim_from = target;
            self.anim_to = target;
            self.have_geometry = true;
        } else if self.pending {
            self.anim_from = self.displayed;
            self.anim_to = target;
            self.indicator =
                AnimationController::new(INDICATOR_DURATION).with_curve(INDICATOR_CURVE);
            self.indicator.forward();
            self.pending = false;
        } else if self.indicator.is_animating() {
            // A relayout that shifted geometry mid-slide: keep the start,
            // re-aim at the new target so the indicator still lands correctly.
            self.anim_to = target;
        } else {
            self.displayed = target;
            self.anim_from = target;
            self.anim_to = target;
        }

        bc.constrain(Size::new(total_width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (accent, _inactive, border) = resolve_tab_colors(theme);

        if self.advance_indicator(ctx.frame_time()) {
            ctx.request_frame();
        }

        let origin = ctx.origin();
        let size = ctx.size();

        // Clip the strip to its own bounds so panned-out tabs don't bleed past
        // the viewport edges (a no-op when nothing overflows).
        scene.push_clip(origin, size);

        // Strip bottom hairline, spanning the full width (fixed — not panned).
        scene.fill_rect(
            origin + Vec2::new(0.0, size.height - STRIP_BORDER_H),
            Size::new(size.width, STRIP_BORDER_H),
            border,
        );

        // Labels: each run already carries its layout-baked color (a selection
        // change forces relayout, so the active/inactive tint is up to date).
        // Every slot x is shifted left by the pan offset.
        for entry in &self.tabs {
            let label_y = origin.y + (size.height - entry.label_size.height) / 2.0;
            entry.label.paint(
                Point::new(origin.x + entry.x + TAB_PAD_X - self.scroll_x, label_y),
                scene,
            );
        }

        // The moving underline (accent), at the strip's bottom — panned with the
        // tabs it tracks.
        let (ind_x, ind_w) = self.displayed;
        scene.fill_rect(
            origin + Vec2::new(ind_x - self.scroll_x, size.height - INDICATOR_H),
            Size::new(ind_w, INDICATOR_H),
            accent,
        );

        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Horizontal scroll/wheel deltas pan the strip when it overflows.
        if let InputEvent::Scroll { delta, .. } = event {
            if self.overflow > 0.0 {
                let dx = match delta {
                    ScrollDelta::Lines(x, _) => x * WHEEL_LINE_PX,
                    ScrollDelta::Pixels(x, _) => *x,
                };
                if dx != 0.0 {
                    self.set_scroll_x(self.scroll_x + dx);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
            }
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                // Only a primary press arms a tab tap or a pan; a secondary
                // press is a context gesture and operates neither.
                if !presses(p) {
                    return EventResult::Ignored;
                }
                self.down_start = p.position;
                self.panning = false;
                match self.hit_index(p.position) {
                    Some(i) => {
                        self.pressed = Some(i);
                        self.captured = Some(i);
                        self.down_active = true;
                        ctx.capture_pointer();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    // A press off any tab (a gap or empty trailing space) only
                    // arms a gesture when the strip can pan; otherwise unchanged.
                    None if self.overflow > 0.0 => {
                        self.pressed = None;
                        self.captured = None;
                        self.down_active = true;
                        ctx.capture_pointer();
                        EventResult::Handled
                    }
                    None => EventResult::Ignored,
                }
            }
            PointerPhase::Move => {
                if !self.down_active {
                    return EventResult::Ignored;
                }
                if self.panning {
                    // Finger right → reveal earlier tabs → scroll_x decreases.
                    let dx = p.position.x - self.last_pan.x;
                    self.last_pan = p.position;
                    self.set_scroll_x(self.scroll_x - dx);
                    ctx.request_redraw();
                } else if self.overflow > 0.0
                    && (p.position.x - self.down_start.x).abs() > TOUCH_SLOP
                {
                    // Crossed the slop: take the gesture over as a pan and drop
                    // any armed tab tap so the release won't fire on_select.
                    self.panning = true;
                    self.pressed = None;
                    self.last_pan = p.position;
                    ctx.request_redraw();
                } else if let Some(cap) = self.captured {
                    self.pressed = (self.hit_index(p.position) == Some(cap)).then_some(cap);
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.down_active {
                    return EventResult::Ignored;
                }
                // A sub-slop press-and-release inside the armed tab still selects
                // it; a pan (or a release off the tab) fires nothing.
                if !self.panning
                    && let Some(cap) = self.captured
                    && self.hit_index(p.position) == Some(cap)
                {
                    (self.on_select)(ctx, cap);
                }
                self.pressed = None;
                self.captured = None;
                self.down_active = false;
                self.panning = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.down_active {
                    return EventResult::Ignored;
                }
                self.pressed = None;
                self.captured = None;
                self.down_active = false;
                self.panning = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for (i, entry) in self.tabs.iter().enumerate() {
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(entry.text.as_str());
                        node.set_selected(i == self.selected);
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::PointerButton;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let peniko::Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn build(view: &TabsView<Vec<usize>>) -> TabsWidget {
        let mut counter = 0u64;
        View::<Vec<usize>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn labels() -> Vec<String> {
        vec![
            "panes".into(),
            "tabs".into(),
            "sessions".into(),
            "logs".into(),
        ]
    }

    fn layout(w: &mut TabsWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(800.0, 100.0)))
    }

    fn paint(w: &mut TabsWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut pctx, &mut rec);
        rec
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn indicator_seeds_under_the_initially_selected_tab() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 1, |_s, _i| {});
        let mut w = build(&view);
        layout(&mut w, None);
        // No animation yet — the indicator sits exactly on tab 1.
        assert!(!w.indicator.is_animating());
        assert_eq!(w.displayed, (w.tabs[1].x, w.tabs[1].width));
    }

    #[test]
    fn indicator_animates_between_positions_and_lands_exactly_under_active_tab() {
        let prev: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&prev);
        layout(&mut w, None);
        let start = w.displayed;
        assert_eq!(start, (w.tabs[0].x, w.tabs[0].width));

        // App confirms selection moved to tab 2.
        let next: TabsView<Vec<usize>> = tabs(labels(), 2, |_s, _i| {});
        let mut counter = 0u64;
        View::<Vec<usize>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        layout(&mut w, None);
        let target = (w.tabs[2].x, w.tabs[2].width);

        // Seed the clock, then advance a partial frame — the indicator is
        // strictly between the two tab positions.
        assert!(w.advance_indicator(ft_ms(0.0)));
        assert!(w.advance_indicator(ft_ms(80.0)));
        let mid = w.displayed;
        assert!(
            mid.0 > start.0 && mid.0 < target.0 + 1.0,
            "x moved toward target: {mid:?}"
        );
        assert_ne!(mid, start);
        assert_ne!(mid, target);

        // Past the duration: settles exactly under tab 2, no longer animating.
        assert!(!w.advance_indicator(ft_ms(500.0)));
        assert_eq!(w.displayed, target);
        assert!(!w.indicator.is_animating());
    }

    #[test]
    fn selection_is_controlled_never_self_mutating() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, None);

        let mut log: Vec<usize> = Vec::new();
        let state_any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, w.height));
        // Tap tab 2 (its computed rect).
        let cx = w.tabs[2].x + w.tabs[2].width / 2.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, cx, w.height / 2.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, cx, w.height / 2.0));
        assert_eq!(log, vec![2]);
        // The widget's own selected index is unchanged — controlled.
        assert_eq!(w.selected, 0);
    }

    #[test]
    fn cancel_clears_press_without_firing() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, None);
        let mut log: Vec<usize> = Vec::new();
        let state_any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, w.height));
        let cx = w.tabs[1].x + w.tabs[1].width / 2.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, cx, w.height / 2.0));
        assert_eq!(w.captured, Some(1));
        w.event(&mut ctx, &ev(PointerPhase::Cancel, cx, w.height / 2.0));
        assert_eq!(w.captured, None);
        w.event(&mut ctx, &ev(PointerPhase::Up, cx, w.height / 2.0));
        assert!(log.is_empty());
    }

    #[test]
    fn unthemed_paint_uses_fallback_accent_and_border() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        // First rect is the strip hairline (border), last is the indicator (accent).
        assert_eq!(rec.rects.first().unwrap().2, TAB_STRIP_BORDER);
        assert_eq!(rec.rects.last().unwrap().2, TAB_ACCENT);
        // The active tab's label is the accent; the rest muted.
        assert_eq!(rec.glyph_colors[0], TAB_ACCENT);
        assert_eq!(rec.glyph_colors[1], TAB_INACTIVE);
    }

    #[test]
    fn glyph_dark_theme_resolves_accent_and_border() {
        let theme = crate::baseline();
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rects.first().unwrap().2, scheme.outline_variant);
        assert_eq!(rec.rects.last().unwrap().2, scheme.primary);
        assert_eq!(rec.glyph_colors[0], scheme.primary);
    }

    #[test]
    fn glyph_light_theme_resolves_accent_and_border() {
        let theme = crate::baseline().with_brightness(frust::Brightness::Light);
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rects.last().unwrap().2, scheme.primary);
        assert_eq!(rec.rects.first().unwrap().2, scheme.outline_variant);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_s: &mut ()) -> TabsView<()> {
            tabs(
                vec!["panes".into(), "tabs".into(), "logs".into()],
                1,
                |_s: &mut (), _i| {},
            )
        }
        let mut root: frust_core::RenderRoot<(), TabsView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 60.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();

        let tablist = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TabList)
            .expect("a TabList container node");
        assert_eq!(tablist.1.children().len(), 3);

        let selected = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Tab && n.label() == Some("tabs"))
            .expect("the selected tab");
        assert_eq!(selected.1.is_selected(), Some(true));
        let unselected = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Tab && n.label() == Some("panes"))
            .expect("an unselected tab");
        assert_eq!(unselected.1.is_selected(), Some(false));
    }

    // ---- Overflow panning (cf1) --------------------------------------------

    /// Lay out into a `width`-wide viewport (loose height), the seam for the
    /// overflow-panning tests.
    fn layout_w(w: &mut TabsWidget, theme: Option<&Theme>, width: f64) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 100.0)))
    }

    /// A horizontal (x) precision scroll of `dx` logical px.
    fn scroll_x_ev(dx: f64) -> InputEvent {
        InputEvent::Scroll {
            position: Point::new(1.0, 1.0),
            delta: ScrollDelta::Pixels(dx, 0.0),
        }
    }

    /// Dispatch `event` at `w` over a throwaway `Vec<usize>` state, returning the
    /// selection log the callback appended to.
    fn dispatch(w: &mut TabsWidget, events: &[InputEvent]) -> Vec<usize> {
        let mut log: Vec<usize> = Vec::new();
        let sa: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(w.viewport_w, w.height));
        for e in events {
            w.event(&mut ctx, e);
        }
        log
    }

    #[test]
    fn narrow_width_overflows_and_pan_clamps_to_range() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        layout_w(&mut w, None, 30.0);
        assert!(w.overflow > 0.0, "a 30px viewport overflows four tabs");
        assert_eq!(w.scroll_x, 0.0, "starts unpanned");

        // A large positive wheel delta clamps to the overflow (not past the end).
        dispatch(&mut w, &[scroll_x_ev(10_000.0)]);
        assert_eq!(w.scroll_x, w.overflow);
        // A large negative delta clamps back to the start.
        dispatch(&mut w, &[scroll_x_ev(-10_000.0)]);
        assert_eq!(w.scroll_x, 0.0);
    }

    #[test]
    fn drag_past_slop_pans_and_does_not_select() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut w = build(&view);
        layout_w(&mut w, None, 30.0);
        let hy = w.height / 2.0;
        // Down on tab 0, cross the slop leftward (takeover, no scroll yet), then
        // drag further left to actually pan, and release.
        let log = dispatch(
            &mut w,
            &[
                ev(PointerPhase::Down, 25.0, hy),
                ev(PointerPhase::Move, 0.0, hy),
                ev(PointerPhase::Move, -30.0, hy),
                ev(PointerPhase::Up, -30.0, hy),
            ],
        );
        assert!(w.scroll_x > 0.0, "the drag panned the strip");
        assert!(log.is_empty(), "a pan never fires on_select");
        assert_eq!(w.selected, 0, "controlled selection is untouched");
    }

    #[test]
    fn sub_slop_release_still_selects() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut w = build(&view);
        layout_w(&mut w, None, 30.0);
        let cx = w.tabs[0].x + w.tabs[0].width / 2.0;
        let hy = w.height / 2.0;
        let log = dispatch(
            &mut w,
            &[
                ev(PointerPhase::Down, cx, hy),
                // A tiny jiggle under the slop must not become a pan.
                ev(PointerPhase::Move, cx + 5.0, hy),
                ev(PointerPhase::Up, cx, hy),
            ],
        );
        assert_eq!(w.scroll_x, 0.0, "a sub-slop press does not pan");
        assert_eq!(log, vec![0], "the tab tap still fires");
    }

    #[test]
    fn hit_test_respects_scroll_x() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        layout_w(&mut w, None, 30.0);
        // Pan so tab 1's left edge sits at the viewport origin.
        w.set_scroll_x(w.tabs[1].x);
        assert_eq!(w.scroll_x, w.tabs[1].x, "the target pan is within range");
        // A press near the left edge now lands on tab 1, not tab 0.
        assert_eq!(w.hit_index(Point::new(1.0, w.height / 2.0)), Some(1));
    }

    #[test]
    fn confirmed_selection_scrolls_into_view() {
        let prev: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&prev);
        layout_w(&mut w, None, 30.0);
        assert_eq!(w.scroll_x, 0.0);
        // Confirm the last tab as selected → next layout brings it into view.
        let last = labels().len() - 1;
        let next: TabsView<Vec<usize>> = tabs(labels(), last, |_s, _i| {});
        let mut counter = 0u64;
        View::<Vec<usize>>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        layout_w(&mut w, None, 30.0);
        // The last tab is at the content's far right, so it pans fully to the end.
        assert_eq!(w.scroll_x, w.overflow);
        let right = w.tabs[last].x + w.tabs[last].width;
        assert!(
            right <= w.scroll_x + w.viewport_w + 0.01,
            "last tab visible"
        );
    }

    #[test]
    fn wheel_delta_pans_when_overflowing() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        layout_w(&mut w, None, 30.0);
        dispatch(&mut w, &[scroll_x_ev(20.0)]);
        assert_eq!(w.scroll_x, 20.0, "a horizontal wheel delta pans");
    }

    #[test]
    fn fits_case_has_no_overflow_and_ignores_wheel() {
        let view: TabsView<Vec<usize>> = tabs(labels(), 0, |_s, _i| {});
        let mut w = build(&view);
        layout_w(&mut w, None, 800.0);
        assert_eq!(w.overflow, 0.0, "four tabs fit in 800px");
        // A wheel event is ignored and never moves a non-overflowing strip.
        dispatch(&mut w, &[scroll_x_ev(50.0)]);
        assert_eq!(w.scroll_x, 0.0);
    }
}
