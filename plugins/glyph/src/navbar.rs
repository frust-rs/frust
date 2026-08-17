//! [`glyph_nav_bar`]/[`GlyphNavBarView`]: the Glyph bottom navigation bar — a
//! raised, bordered bar of 2-5 destinations, each a glyph/icon above a tiny
//! `9.5px` micro label, the active one tinted accent (the Glyph design
//! system's `.bottom-nav-demo`/`.bottom-nav-item`/
//! `.bottom-nav-item.active`/`.bottom-nav-glyph` rules, retrieved 2026-07-21).
//!
//! # Controlled selection
//!
//! Like `crate::material::navigation_bar` and [`crate::tabs`], this is
//! a **controlled** component: it reports the requested index through
//! `on_select(index)` and never mutates its own `selected`; the app feeds the
//! confirmed index back in on the next rebuild.
//!
//! # SafeArea
//!
//! This widget does **no** internal inset handling — matching the precedent of
//! the existing bottom bars (`crate::material::navbar` lays out at its fixed
//! metrics and leaves system-bar/gesture-nav insets to the caller). Compose it
//! with `crate::SafeArea` (the sole consumer of the inset channel — see
//! `docs/ARCHITECTURE.md`'s Inset delivery) when the bar must clear the
//! platform's bottom inset.
//!
//! # Why this widget shapes its own runs
//!
//! Each destination's glyph and label carry a per-item dynamic color (accent
//! when selected, muted otherwise) that isn't one of `TextView`'s four fixed
//! roles, so this widget shapes/paints them directly via `frust_text` — see
//! [`crate::badge`]'s module docs for the full rationale.

use std::rc::Rc;

use frust::IconData;
use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust::{ShapeScale, Theme};
use kurbo::{Point, Rect, Size};
use peniko::{Brush, Color};

/// Container vertical padding, logical px (`.bottom-nav-demo{padding:12px 8px}`
/// → 12px vertical; the 8px horizontal padding emerges from the even
/// slot-centering below, so it is not a separate metric here).
const NAV_PAD_Y: f64 = 12.0;
/// Gap between an item's glyph and its label, logical px
/// (`.bottom-nav-item{gap:5px}`).
const NAV_ITEM_GAP: f64 = 5.0;
/// Glyph font size, logical px (`.bottom-nav-glyph{font-size:15px}`).
const NAV_GLYPH_SIZE: f32 = 15.0;
/// Micro-label font size, logical px (`.bottom-nav-item{font-size:9.5px}`).
const NAV_LABEL_SIZE: f32 = 9.5;
/// Container border width, logical px (`.bottom-nav-demo{border:1px}`).
const NAV_BORDER_W: f64 = 1.0;
/// Corner-rounding tolerance for the border stroke (matches the sibling Glyph
/// widgets' `PATH_TOLERANCE`).
const NAV_PATH_TOLERANCE: f64 = 0.1;

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// Container fill — `--bg-surface` / themed `surface_container`.
const NAV_CONTAINER_BG: Color = Color::from_rgb8(0x16, 0x1a, 0x23);
/// Container border — `--border-bright` / themed `outline`.
const NAV_CONTAINER_BORDER: Color = Color::from_rgb8(0x3e, 0x3f, 0x44);
/// Active item tint — `--amber` / themed `primary`.
const NAV_ACTIVE: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Inactive item tint — `--fg-dim` / themed `on_surface_variant`.
const NAV_INACTIVE: Color = Color::from_rgb8(0x6b, 0x65, 0x56);

/// Resolve `(container-bg, container-border, active, inactive)` from the theme,
/// falling back to the literal Glyph **dark** constants with no theme threaded.
fn resolve_nav_colors(theme: Option<&Theme>) -> (Color, Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.surface_container,
                scheme.outline,
                scheme.primary,
                scheme.on_surface_variant,
            )
        }
        None => (
            NAV_CONTAINER_BG,
            NAV_CONTAINER_BORDER,
            NAV_ACTIVE,
            NAV_INACTIVE,
        ),
    }
}

/// Container radius (`--radius-lg`, 16px) — themed `shape.large`, else the
/// literal fallback.
fn nav_radius(theme: Option<&Theme>, size: Size) -> f64 {
    match theme {
        Some(theme) => ShapeScale::resolve(theme.shape.large, size.width, size.height),
        None => 16.0,
    }
}

/// One declarative destination: a glyph (char **or** vector icon) above a
/// micro label.
pub struct GlyphNavItem {
    glyph: NavItemGlyph,
    label: String,
}

/// The two glyph faces a [`GlyphNavItem`] can render.
///
/// **Cross-platform caveat for `Char`:** a character outside the bundled
/// Glyph fonts' coverage (IBM Plex Mono ships box-drawing U+2500–259F and
/// `◊`, but NO Geometric Shapes circles/diamonds) resolves through the
/// *platform's* system-font fallback — broad on Android, narrow on iOS,
/// where uncovered codepoints render as tofu boxes. For chrome that must
/// look identical everywhere, prefer [`glyph_nav_item_icon`] (a
/// deterministic vector path) or restrict chars to the bundled coverage.
enum NavItemGlyph {
    Char(String),
    Icon(IconData),
}

/// Create a nav item rendering the `glyph` char above `label` (see
/// [`NavItemGlyph`]'s font-coverage caveat).
pub fn glyph_nav_item(glyph: impl Into<String>, label: impl Into<String>) -> GlyphNavItem {
    GlyphNavItem {
        glyph: NavItemGlyph::Char(glyph.into()),
        label: label.into(),
    }
}

/// Create a nav item rendering a vector `icon` above `label` — the
/// platform-deterministic alternative to a glyph char (no font-fallback
/// dependency; same pixels on every OS). The icon paints as a filled path
/// (the [`frust::icon`] contract) tinted with the item's active/inactive
/// color, scaled into the same box a glyph char occupies.
pub fn glyph_nav_item_icon(icon: impl Into<IconData>, label: impl Into<String>) -> GlyphNavItem {
    GlyphNavItem {
        glyph: NavItemGlyph::Icon(icon.into()),
        label: label.into(),
    }
}

impl NavItemGlyph {
    /// Cheap same-glyph check for [`GlyphNavBarView::rebuild`]'s structural
    /// diff — string equality for chars, [`IconData::same`] for icons (Arc
    /// pointer / static-source identity, so a memoized app icon is stable
    /// across per-frame rebuilds).
    fn same(&self, other: &NavItemGlyph) -> bool {
        match (self, other) {
            (NavItemGlyph::Char(a), NavItemGlyph::Char(b)) => a == b,
            (NavItemGlyph::Icon(a), NavItemGlyph::Icon(b)) => a.same(b),
            _ => false,
        }
    }
}

/// A view-held, typed selection callback.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative Glyph bottom navigation bar. See the [module docs](self).
pub struct GlyphNavBarView<State: 'static> {
    items: Vec<GlyphNavItem>,
    selected: usize,
    on_select: OnSelect<State>,
}

/// Create a bottom nav bar over `items` (2-5 destinations), with `selected` the
/// current (app-confirmed) index. Fires `on_select(state, index)` on a release
/// inside an item — a **controlled** component.
pub fn glyph_nav_bar<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<GlyphNavItem>,
    selected: usize,
    on_select: F,
) -> GlyphNavBarView<State> {
    GlyphNavBarView {
        items,
        selected,
        on_select: Rc::new(on_select),
    }
}

/// A minimal retained text run — see [`crate::badge`]'s `GlyphLabel`.
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

/// A retained glyph slot: a shaped char run or a resolved vector icon.
enum NavSlot {
    Char(Box<GlyphLabel>),
    Icon(IconData),
}

impl NavSlot {
    fn layout(&mut self, ctx: &mut LayoutCtx, tint: Color) -> Size {
        match self {
            NavSlot::Char(label) => label.layout(ctx, &glyph_style(tint), None),
            // A vector icon occupies the same square box a glyph char's face
            // size defines — no text shaping involved.
            NavSlot::Icon(_) => Size::new(NAV_GLYPH_SIZE as f64, NAV_GLYPH_SIZE as f64),
        }
    }

    fn paint(&self, origin: Point, tint: Color, scene: &mut dyn PaintScene) {
        match self {
            NavSlot::Char(label) => label.paint(origin, scene),
            NavSlot::Icon(data) => {
                let (path, design) = data.resolve();
                let scale = if design > 0.0 {
                    NAV_GLYPH_SIZE as f64 / design
                } else {
                    1.0
                };
                let scaled = kurbo::Affine::scale(scale) * path;
                scene.fill_path(origin, &scaled, &Brush::Solid(tint));
            }
        }
    }
}

/// One retained destination.
struct NavEntry {
    glyph: NavSlot,
    label: GlyphLabel,
    label_text: String,
    glyph_size: Size,
    label_size: Size,
    /// Local x of this item's slot's left edge (filled at layout).
    slot_x: f64,
    /// This item's slot width (filled at layout).
    slot_w: f64,
}

/// The glyph face style (larger, `.bottom-nav-glyph`).
fn glyph_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(NAV_GLYPH_SIZE, color)
    }
}

/// The micro-label style.
fn label_style(color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(NAV_LABEL_SIZE, color)
    }
}

fn build_items(items: &[GlyphNavItem]) -> Vec<NavEntry> {
    items
        .iter()
        .map(|item| NavEntry {
            glyph: match &item.glyph {
                NavItemGlyph::Char(s) => NavSlot::Char(Box::new(GlyphLabel::new(s.clone()))),
                NavItemGlyph::Icon(data) => NavSlot::Icon(data.clone()),
            },
            label: GlyphLabel::new(item.label.clone()),
            label_text: item.label.clone(),
            glyph_size: Size::ZERO,
            label_size: Size::ZERO,
            slot_x: 0.0,
            slot_w: 0.0,
        })
        .collect()
}

/// The retained widget for a [`GlyphNavBarView`].
pub struct GlyphNavBarWidget {
    items: Vec<NavEntry>,
    selected: usize,
    on_select: frust::authoring::ErasedArgCallback<usize>,
    height: f64,
    pressed: Option<usize>,
    captured: Option<usize>,
}

impl<State: 'static> View<State> for GlyphNavBarView<State> {
    type Element = GlyphNavBarWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GlyphNavBarWidget {
        GlyphNavBarWidget {
            items: build_items(&self.items),
            selected: self.selected,
            on_select: frust::authoring::erase_callback_arg(&self.on_select),
            height: 0.0,
            pressed: None,
            captured: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GlyphNavBarWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = frust::authoring::erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;
        let structural = prev.items.len() != self.items.len()
            || prev
                .items
                .iter()
                .zip(self.items.iter())
                .any(|(p, n)| !p.glyph.same(&n.glyph) || p.label != n.label);
        if structural {
            element.items = build_items(&self.items);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.selected != self.selected {
            element.selected = self.selected;
            // Item tints are layout-time-baked, so a selection change must force
            // relayout to re-shape the new active/inactive colors.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }
}

impl GlyphNavBarWidget {
    /// The destination index whose slot contains a widget-local pointer
    /// position, if any.
    fn hit_index(&self, pos: Point) -> Option<usize> {
        self.items.iter().position(|e| {
            Rect::from_origin_size(Point::new(e.slot_x, 0.0), Size::new(e.slot_w, self.height))
                .contains(pos)
        })
    }
}

impl Widget for GlyphNavBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (_, _, active, inactive) = resolve_nav_colors(theme);

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let n = self.items.len();
        let slot_w = if n > 0 { width / n as f64 } else { 0.0 };

        let mut content_height = 0.0_f64;
        for (i, entry) in self.items.iter_mut().enumerate() {
            let tint = if i == self.selected { active } else { inactive };
            let glyph_size = entry.glyph.layout(ctx, tint);
            let label_size = entry.label.layout(ctx, &label_style(tint), None);
            entry.glyph_size = glyph_size;
            entry.label_size = label_size;
            entry.slot_x = i as f64 * slot_w;
            entry.slot_w = slot_w;
            let item_h = glyph_size.height + NAV_ITEM_GAP + label_size.height;
            content_height = content_height.max(item_h);
        }
        let height = content_height + NAV_PAD_Y * 2.0;
        self.height = height;
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (bg, border, active, inactive) = resolve_nav_colors(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        // Container: raised fill + hairline border.
        let radius = nav_radius(theme, size);
        scene.fill_rounded_rect(origin, size, radius, bg);
        let rr = kurbo::RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, size), radius);
        scene.stroke_path(
            origin,
            &kurbo::Shape::to_path(&rr, NAV_PATH_TOLERANCE),
            NAV_BORDER_W,
            &Brush::Solid(border),
        );

        for (i, entry) in self.items.iter().enumerate() {
            let tint = if i == self.selected { active } else { inactive };
            // Vertically center the glyph+gap+label stack.
            let stack_h = entry.glyph_size.height + NAV_ITEM_GAP + entry.label_size.height;
            let top = origin.y + (size.height - stack_h) / 2.0;
            let slot_center = origin.x + entry.slot_x + entry.slot_w / 2.0;

            let glyph_x = slot_center - entry.glyph_size.width / 2.0;
            entry.glyph.paint(Point::new(glyph_x, top), tint, scene);

            let label_x = slot_center - entry.label_size.width / 2.0;
            let label_y = top + entry.glyph_size.height + NAV_ITEM_GAP;
            entry.label.paint(Point::new(label_x, label_y), scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => match self.hit_index(p.position) {
                Some(i) => {
                    self.pressed = Some(i);
                    self.captured = Some(i);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                None => EventResult::Ignored,
            },
            PointerPhase::Move => {
                let Some(cap) = self.captured else {
                    return EventResult::Ignored;
                };
                self.pressed = (self.hit_index(p.position) == Some(cap)).then_some(cap);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(cap) = self.captured else {
                    return EventResult::Ignored;
                };
                if self.hit_index(p.position) == Some(cap) {
                    (self.on_select)(ctx, cap);
                }
                self.pressed = None;
                self.captured = None;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.is_none() {
                    return EventResult::Ignored;
                }
                self.pressed = None;
                self.captured = None;
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
                for (i, entry) in self.items.iter().enumerate() {
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(entry.label_text.as_str());
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
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        glyph_colors: Vec<Color>,
        path_fills: Vec<(Point, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn fill_path(&mut self, o: Point, _p: &kurbo::BezPath, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.path_fills.push((o, *c));
            }
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn items() -> Vec<GlyphNavItem> {
        vec![
            glyph_nav_item("▣", "panes"),
            glyph_nav_item("◧", "tabs"),
            glyph_nav_item("◎", "sessions"),
            glyph_nav_item("⚙", "settings"),
        ]
    }

    fn build(view: &GlyphNavBarView<Vec<usize>>) -> GlyphNavBarWidget {
        let mut counter = 0u64;
        View::<Vec<usize>>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut GlyphNavBarWidget, size: Size, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(size))
    }

    fn paint(w: &mut GlyphNavBarWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        let mut rec = Recorder::default();
        let mut pctx = match theme {
            Some(t) => PaintCtx::new(Point::ZERO, size).with_theme(t),
            None => PaintCtx::new(Point::ZERO, size),
        };
        w.paint(&mut pctx, &mut rec);
        rec
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn layout_distributes_slots_evenly_across_width() {
        let view: GlyphNavBarView<Vec<usize>> = glyph_nav_bar(items(), 0, |_s, _i| {});
        let mut w = build(&view);
        layout(&mut w, Size::new(360.0, 100.0), None);
        assert_eq!(w.items[0].slot_x, 0.0);
        assert_eq!(w.items[1].slot_x, 90.0);
        assert_eq!(w.items[2].slot_x, 180.0);
        assert_eq!(w.items[3].slot_x, 270.0);
        assert_eq!(w.items[0].slot_w, 90.0);
    }

    #[test]
    fn unthemed_paint_uses_fallback_container_and_tints() {
        let view: GlyphNavBarView<Vec<usize>> = glyph_nav_bar(items(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(360.0, 100.0), None);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, NAV_CONTAINER_BG);
        assert_eq!(rec.strokes[0], NAV_CONTAINER_BORDER);
        // Item 0 active → its glyph (first run) is accent.
        assert_eq!(rec.glyph_colors[0], NAV_ACTIVE);
        // Item 1's glyph (third run: glyph0,label0,glyph1,...) is muted.
        assert_eq!(rec.glyph_colors[2], NAV_INACTIVE);
    }

    #[test]
    fn glyph_dark_theme_resolves_roles() {
        let theme = crate::baseline();
        let view: GlyphNavBarView<Vec<usize>> = glyph_nav_bar(items(), 2, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(360.0, 100.0), Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.surface_container);
        assert_eq!(rec.strokes[0], scheme.outline);
        // Item 2 active → its glyph is the 5th run (glyph0,label0,glyph1,label1,glyph2).
        assert_eq!(rec.glyph_colors[4], scheme.primary);
    }

    #[test]
    fn glyph_light_theme_resolves_container() {
        let theme = crate::baseline().with_brightness(frust::Brightness::Light);
        let view: GlyphNavBarView<Vec<usize>> = glyph_nav_bar(items(), 0, |_s, _i| {});
        let mut w = build(&view);
        let size = layout(&mut w, Size::new(360.0, 100.0), Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(rec.rrects[0].3, theme.scheme().surface_container);
    }

    #[test]
    fn tap_reports_index_without_self_mutating() {
        let view: GlyphNavBarView<Vec<usize>> =
            glyph_nav_bar(items(), 0, |s: &mut Vec<usize>, i| s.push(i));
        let mut counter = 0u64;
        let mut w = View::<Vec<usize>>::build(&view, &mut BuildCtx::new(&mut counter));
        layout(&mut w, Size::new(360.0, 100.0), None);
        let mut log: Vec<usize> = Vec::new();
        let state_any: &mut dyn Any = &mut log;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(360.0, w.height));
        // Item 2's slot is x in [180, 270).
        w.event(&mut ctx, &ev(PointerPhase::Down, 200.0, w.height / 2.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, 200.0, w.height / 2.0));
        assert_eq!(log, vec![2]);
        assert_eq!(w.selected, 0);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_with_selection() {
        fn logic(_s: &mut ()) -> GlyphNavBarView<()> {
            glyph_nav_bar(
                vec![
                    glyph_nav_item("▣", "panes"),
                    glyph_nav_item("◧", "tabs"),
                    glyph_nav_item("⚙", "settings"),
                ],
                1,
                |_s: &mut (), _i| {},
            )
        }
        let mut root: frust_core::RenderRoot<(), GlyphNavBarView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(360.0, 72.0), &mut tcx as &mut dyn Any);
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
            .expect("the selected item");
        assert_eq!(selected.1.is_selected(), Some(true));
    }

    // --- Vector-icon items (glyph_nav_item_icon): platform-deterministic
    //     path fills instead of font-fallback-dependent glyph chars. ---

    fn diamond_icon() -> IconData {
        // A 10×10-design filled diamond (the `◆` shape as a path).
        let mut p = kurbo::BezPath::new();
        p.move_to((5.0, 0.0));
        p.line_to((10.0, 5.0));
        p.line_to((5.0, 10.0));
        p.line_to((0.0, 5.0));
        p.close_path();
        IconData::from_path(p, 10.0)
    }

    #[test]
    fn icon_items_paint_path_fills_with_item_tints_and_fixed_glyph_box() {
        let view: GlyphNavBarView<Vec<usize>> = glyph_nav_bar(
            vec![
                glyph_nav_item_icon(diamond_icon(), "home"),
                glyph_nav_item_icon(diamond_icon(), "you"),
            ],
            0,
            |_s, _i| {},
        );
        let mut w = build(&view);
        let theme = crate::baseline();
        layout(&mut w, Size::new(360.0, 200.0), Some(&theme));
        // Icon slots use the fixed glyph box, no text shaping.
        assert_eq!(
            w.items[0].glyph_size,
            Size::new(NAV_GLYPH_SIZE as f64, NAV_GLYPH_SIZE as f64)
        );
        let rec = paint(&mut w, Size::new(360.0, 46.0), Some(&theme));
        // One filled path per icon item; the selected item carries the active
        // tint, the other the inactive tint (both from resolve_nav_colors).
        assert_eq!(rec.path_fills.len(), 2, "one fill_path per icon item");
        let (_, _, active, inactive) = resolve_nav_colors(Some(&theme));
        assert_eq!(rec.path_fills[0].1, active, "selected item tint");
        assert_eq!(rec.path_fills[1].1, inactive, "unselected item tint");
    }

    #[test]
    fn icon_item_rebuild_is_stable_for_a_memoized_icon() {
        // The same IconData handle (Arc identity) must NOT read as structural
        // on a per-frame rebuild; a fresh path (new Arc) must.
        let shared = diamond_icon();
        let mk = |icon: IconData| -> GlyphNavBarView<Vec<usize>> {
            glyph_nav_bar(
                vec![glyph_nav_item_icon(icon, "home")],
                0,
                |_s: &mut Vec<usize>, _i| {},
            )
        };
        let a = mk(shared.clone());
        let b = mk(shared.clone());
        let mut w = build(&a);
        let mut counter = 0u64;
        let flags = View::<Vec<usize>>::rebuild(&b, &a, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(flags, ChangeFlags::NONE, "same Arc => not structural");
        let c = mk(diamond_icon());
        let flags = View::<Vec<usize>>::rebuild(&c, &b, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(
            flags.contains(ChangeFlags::LAYOUT),
            "fresh Arc => structural rebuild"
        );
    }
}
