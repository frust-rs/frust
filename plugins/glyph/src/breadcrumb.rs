//! [`breadcrumb`]/[`BreadcrumbView`]: an inline path of tappable crumbs joined
//! by `/` separators, the last crumb being the current (non-link) location
//! (the Glyph design system's `.breadcrumb`/`.breadcrumb a`/
//! `.breadcrumb .sep`/`.breadcrumb .current` rules, retrieved 2026-07-21).
//!
//! Each non-final crumb fires its own per-crumb callback on release-inside
//! (fire-on-up-inside, like every interactive widget in this crate — see
//! `docs/CODE_STANDARDS.md`'s Interaction Semantics). The final crumb is the
//! current location: painted in the primary foreground and never tappable.
//!
//! # Why this widget shapes its own runs
//!
//! See [`crate::badge`]'s module docs — the per-crumb dynamic color
//! (link vs current vs separator) isn't one of `TextView`'s four fixed roles,
//! so this widget shapes/paints its own glyph runs directly via `frust_text`.
//!
//! Every run (crumbs and separators alike) reads its family at layout from
//! the live theme's `labelMedium` type-scale role (IBM Plex Mono under
//! Glyph's own scale), falling back to Glyph's IBM Plex Mono stack unthemed,
//! so a theme swap reshapes the path (see [`crate::badge`]'s Typeface
//! section).

use std::rc::Rc;

use frust::Theme;
use frust::authoring::Role;
use frust::authoring::text::{
    FontFamily, FontWeight, GenericSlot, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use kurbo::{Point, Rect, Size};
use peniko::Color;

use crate::press::presses;

/// Gap between adjacent crumbs and separators, logical px (`.breadcrumb{gap:8px}`).
const CRUMB_GAP: f64 = 8.0;
/// Crumb + separator font size, logical px (`.breadcrumb{font-size:11.5px}`).
const CRUMB_FONT_SIZE: f32 = 11.5;
/// The separator glyph (`.breadcrumb .sep` renders `/`).
const CRUMB_SEP: &str = "/";

// ---- Unthemed fallback constants (Glyph **dark** values) -------------------

/// Link crumb tone — `--fg-muted` / themed `on_surface_variant`.
const CRUMB_LINK: Color = Color::from_rgb8(0xa3, 0x9c, 0x88);
/// Current crumb tone — `--fg` / themed `on_surface`.
const CRUMB_CURRENT: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);
/// Separator tone — `--fg-faintest` / themed `outline_variant`.
const CRUMB_SEPARATOR: Color = Color::from_rgb8(0x33, 0x2f, 0x26);

/// Resolve `(link, current, separator)` from the theme, falling back to the
/// literal Glyph **dark** constants with no theme threaded.
fn resolve_crumb_colors(theme: Option<&Theme>) -> (Color, Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (
                scheme.on_surface_variant,
                scheme.on_surface,
                scheme.outline_variant,
            )
        }
        None => (CRUMB_LINK, CRUMB_CURRENT, CRUMB_SEPARATOR),
    }
}

/// A view-held, typed per-crumb tap callback.
type OnTap<State> = Rc<dyn Fn(&mut State)>;

/// One declarative crumb: a label with an optional tap callback. See
/// [`crumb`].
pub struct Crumb<State: 'static> {
    label: String,
    on_tap: Option<OnTap<State>>,
}

/// Create a crumb labelled `label` with no tap callback; chain
/// [`Crumb::on_tap`] to make it a link.
pub fn crumb<State: 'static>(label: impl Into<String>) -> Crumb<State> {
    Crumb {
        label: label.into(),
        on_tap: None,
    }
}

impl<State: 'static> Crumb<State> {
    /// Fire `on_tap` against the app state when this crumb is released inside
    /// its bounds (fire-on-up-inside).
    pub fn on_tap<F: Fn(&mut State) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }
}

/// A declarative breadcrumb path. See the [module docs](self).
pub struct BreadcrumbView<State: 'static> {
    crumbs: Vec<Crumb<State>>,
}

/// Create a breadcrumb over `crumbs`; the final crumb is rendered as the
/// current (non-link) location.
pub fn breadcrumb<State: 'static>(crumbs: Vec<Crumb<State>>) -> BreadcrumbView<State> {
    BreadcrumbView { crumbs }
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

/// One retained crumb: its label run, whether it is a link, and its computed
/// local geometry.
struct CrumbEntry {
    label: GlyphLabel,
    text: String,
    is_link: bool,
    x: f64,
    size: Size,
    on_tap: Option<frust::authoring::ErasedCallback>,
}

/// Glyph's UI face stack (IBM Plex Mono): the crumbs' unthemed family.
fn ui_face() -> FontFamily {
    FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace)
}

/// The crumb/separator style: fixed weight/size, family from the theme's
/// `labelMedium` role; color varies per run.
fn crumb_style(theme: Option<&Theme>, color: Color) -> TextStyle {
    TextStyle {
        family: theme.map_or_else(ui_face, |t| t.type_scale.label_medium.family.clone()),
        weight: FontWeight::REGULAR,
        ..TextStyle::new(CRUMB_FONT_SIZE, color)
    }
}

/// The retained widget for a [`BreadcrumbView`].
pub struct BreadcrumbWidget {
    crumbs: Vec<CrumbEntry>,
    /// The retained separator run (one shared shape, re-painted between crumbs).
    separator: GlyphLabel,
    separator_size: Size,
    height: f64,
    pressed: Option<usize>,
    captured: Option<usize>,
}

fn build_crumbs<State: 'static>(crumbs: &[Crumb<State>]) -> Vec<CrumbEntry> {
    let last = crumbs.len().saturating_sub(1);
    crumbs
        .iter()
        .enumerate()
        .map(|(i, c)| CrumbEntry {
            label: GlyphLabel::new(c.label.clone()),
            text: c.label.clone(),
            // The final crumb is the current location, never a link even if a
            // callback was (incorrectly) attached.
            is_link: i != last && c.on_tap.is_some(),
            x: 0.0,
            size: Size::ZERO,
            on_tap: if i != last {
                c.on_tap.as_ref().map(frust::authoring::erase_callback)
            } else {
                None
            },
        })
        .collect()
}

impl<State: 'static> View<State> for BreadcrumbView<State> {
    type Element = BreadcrumbWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> BreadcrumbWidget {
        BreadcrumbWidget {
            crumbs: build_crumbs(&self.crumbs),
            separator: GlyphLabel::new(CRUMB_SEP),
            separator_size: Size::ZERO,
            height: 0.0,
            pressed: None,
            captured: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BreadcrumbWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let structural = prev.crumbs.len() != self.crumbs.len()
            || prev
                .crumbs
                .iter()
                .zip(self.crumbs.iter())
                .any(|(p, n)| p.label != n.label);
        if structural {
            element.crumbs = build_crumbs(&self.crumbs);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            // Labels unchanged — only reinstall the (uncomparable) callbacks.
            let last = self.crumbs.len().saturating_sub(1);
            for (i, (entry, spec)) in element
                .crumbs
                .iter_mut()
                .zip(self.crumbs.iter())
                .enumerate()
            {
                entry.on_tap = if i != last {
                    spec.on_tap.as_ref().map(frust::authoring::erase_callback)
                } else {
                    None
                };
            }
        }
        flags
    }
}

impl BreadcrumbWidget {
    /// The index of the tappable (link) crumb under a widget-local pointer
    /// position, if any. The current (final) crumb is never a link, so it never
    /// hits.
    fn hit_index(&self, pos: Point) -> Option<usize> {
        self.crumbs.iter().position(|c| {
            c.is_link
                && Rect::from_origin_size(
                    Point::new(c.x, 0.0),
                    Size::new(c.size.width, self.height),
                )
                .contains(pos)
        })
    }
}

impl Widget for BreadcrumbWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve every style up front so the immutable theme borrow ends
        // before the `&mut ctx` shaping calls below.
        let theme = Theme::from_layout_ctx(ctx);
        let (link, current, separator) = resolve_crumb_colors(theme);
        let link_style = crumb_style(theme, link);
        let current_style = crumb_style(theme, current);
        let sep_style = crumb_style(theme, separator);

        let sep_size = self.separator.layout(ctx, &sep_style, None);
        self.separator_size = sep_size;

        let last = self.crumbs.len().saturating_sub(1);
        let mut x = 0.0;
        let mut content_height = sep_size.height;
        for (i, entry) in self.crumbs.iter_mut().enumerate() {
            let style = if i == last {
                &current_style
            } else {
                &link_style
            };
            let size = entry.label.layout(ctx, style, None);
            entry.size = size;
            entry.x = x;
            content_height = content_height.max(size.height);
            x += size.width;
            // A separator follows every crumb but the last.
            if i != last {
                x += CRUMB_GAP + sep_size.width + CRUMB_GAP;
            }
        }
        self.height = content_height;
        bc.constrain(Size::new(x, content_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        let last = self.crumbs.len().saturating_sub(1);
        for (i, entry) in self.crumbs.iter().enumerate() {
            let label_y = origin.y + (size.height - entry.size.height) / 2.0;
            entry
                .label
                .paint(Point::new(origin.x + entry.x, label_y), scene);
            if i != last {
                let sep_x = origin.x + entry.x + entry.size.width + CRUMB_GAP;
                let sep_y = origin.y + (size.height - self.separator_size.height) / 2.0;
                self.separator.paint(Point::new(sep_x, sep_y), scene);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down if !presses(p) => EventResult::Ignored,
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
                if self.hit_index(p.position) == Some(cap)
                    && let Some(on_tap) = self.crumbs[cap].on_tap.as_mut()
                {
                    on_tap(ctx);
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
        let last = self.crumbs.len().saturating_sub(1);
        for (i, entry) in self.crumbs.iter().enumerate() {
            let role = if entry.is_link {
                Role::Link
            } else {
                Role::Label
            };
            let is_current = i == last;
            ctx.push_node(role, |node| {
                node.set_label(entry.text.as_str());
                if is_current {
                    node.set_selected(true);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::PointerButton;
    use std::any::Any;

    #[derive(Default)]
    struct NavState {
        taps: Vec<usize>,
    }

    #[derive(Default)]
    struct Recorder {
        glyph_colors: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let peniko::Brush::Solid(c) = run.brush {
                self.glyph_colors.push(c);
            }
        }
    }

    fn sample() -> BreadcrumbView<NavState> {
        breadcrumb(vec![
            crumb::<NavState>("servers").on_tap(|s: &mut NavState| s.taps.push(0)),
            crumb::<NavState>("dev.home.local").on_tap(|s: &mut NavState| s.taps.push(1)),
            crumb::<NavState>("settings"),
        ])
    }

    fn build(view: &BreadcrumbView<NavState>) -> BreadcrumbWidget {
        let mut counter = 0u64;
        View::<NavState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut BreadcrumbWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = match theme {
            Some(t) => {
                LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(t as &dyn Any))
            }
            None => LayoutCtx::with_text_context(&mut tcx as &mut dyn Any),
        };
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(800.0, 60.0)))
    }

    fn paint(w: &mut BreadcrumbWidget, size: Size, theme: Option<&Theme>) -> Recorder {
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
    fn last_crumb_is_current_and_not_a_link() {
        let w = build(&sample());
        assert!(w.crumbs[0].is_link);
        assert!(w.crumbs[1].is_link);
        assert!(
            !w.crumbs[2].is_link,
            "the final crumb is the current location"
        );
        assert!(w.crumbs[2].on_tap.is_none());
    }

    #[test]
    fn unthemed_paint_colors_links_current_and_separators() {
        let mut w = build(&sample());
        let size = layout(&mut w, None);
        let rec = paint(&mut w, size, None);
        // Glyph-run order: crumb0(link), sep, crumb1(link), sep, crumb2(current).
        assert_eq!(rec.glyph_colors[0], CRUMB_LINK);
        assert_eq!(rec.glyph_colors[1], CRUMB_SEPARATOR);
        assert_eq!(rec.glyph_colors[2], CRUMB_LINK);
        assert_eq!(rec.glyph_colors[3], CRUMB_SEPARATOR);
        assert_eq!(rec.glyph_colors[4], CRUMB_CURRENT);
    }

    #[test]
    fn glyph_dark_theme_resolves_roles() {
        let theme = crate::baseline();
        let mut w = build(&sample());
        let size = layout(&mut w, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let scheme = theme.scheme();
        assert_eq!(rec.glyph_colors[0], scheme.on_surface_variant);
        assert_eq!(rec.glyph_colors[1], scheme.outline_variant);
        assert_eq!(rec.glyph_colors[4], scheme.on_surface);
    }

    #[test]
    fn tapping_a_link_crumb_fires_its_callback() {
        let mut w = build(&sample());
        layout(&mut w, None);
        let mut state = NavState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, w.height));
        let cx = w.crumbs[1].x + w.crumbs[1].size.width / 2.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, cx, w.height / 2.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, cx, w.height / 2.0));
        assert_eq!(state.taps, vec![1]);
    }

    #[test]
    fn tapping_the_current_crumb_is_ignored() {
        let mut w = build(&sample());
        layout(&mut w, None);
        let mut state = NavState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, w.height));
        let cx = w.crumbs[2].x + w.crumbs[2].size.width / 2.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, cx, w.height / 2.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, cx, w.height / 2.0));
        assert!(state.taps.is_empty());
    }

    #[test]
    fn cancel_clears_press_without_firing() {
        let mut w = build(&sample());
        layout(&mut w, None);
        let mut state = NavState::default();
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(400.0, w.height));
        let cx = w.crumbs[0].x + w.crumbs[0].size.width / 2.0;
        w.event(&mut ctx, &ev(PointerPhase::Down, cx, w.height / 2.0));
        assert_eq!(w.captured, Some(0));
        w.event(&mut ctx, &ev(PointerPhase::Cancel, cx, w.height / 2.0));
        w.event(&mut ctx, &ev(PointerPhase::Up, cx, w.height / 2.0));
        assert!(state.taps.is_empty());
    }

    #[test]
    fn semantics_reports_links_and_a_current_label() {
        fn logic(_s: &mut ()) -> BreadcrumbView<()> {
            breadcrumb(vec![
                crumb::<()>("servers").on_tap(|_s: &mut ()| {}),
                crumb::<()>("settings"),
            ])
        }
        let mut root: frust_core::RenderRoot<(), BreadcrumbView<()>> =
            frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 40.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let link = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Link)
            .expect("a link crumb");
        assert_eq!(link.1.label(), Some("servers"));
        let current = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Label && n.label() == Some("settings"))
            .expect("the current crumb");
        assert_eq!(current.1.is_selected(), Some(true));
    }

    // ---- Typeface: every run's family follows its type-scale role ---------

    #[cfg(feature = "bundled-fonts")]
    fn two_crumbs(_: &mut ()) -> BreadcrumbView<()> {
        breadcrumb(vec![
            crumb::<()>("servers").on_tap(|_s: &mut ()| {}),
            crumb::<()>("settings"),
        ])
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_paint_in_their_role_face_under_the_glyph_theme() {
        use crate::badge::typeface_probe::{Face, painted_faces};
        let faces = painted_faces(two_crumbs, crate::baseline(), Size::new(400.0, 40.0));
        // Link crumb, separator, current crumb.
        assert_eq!(faces, [Face::PlexMono; 3]);
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn runs_follow_a_live_theme_family_swap() {
        use crate::badge::typeface_probe::{Face, faces_across_a_live_swap};
        let (before, after) = faces_across_a_live_swap(two_crumbs, Size::new(400.0, 40.0));
        assert_eq!(before, [Face::PlexMono; 3]);
        assert_eq!(after, [Face::SpaceMono; 3]);
    }
}
