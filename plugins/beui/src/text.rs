//! The catalog's shared cached text runs.
//!
//! Every text-bearing component in this catalog shapes its own label rather
//! than nesting a `frust::text` child — the framework's baseline text widget
//! bakes its color into the layout, and a component wants either a color of
//! its own choosing ([`Label`]) or a color that changes on a repaint-only
//! event without a relayout ([`LabelRun`]). Both cache the shaped layout and
//! re-shape only when the text or the [`TextStyle`] it was shaped with
//! actually changes — the same contract `plugins/shadcn/src/text.rs`'s
//! `Label`/`LabelRun` pair establishes for that catalog.
//!
//! # Which one a component wants
//!
//! - [`Label`] — the color is part of the style, baked in at layout time.
//!   Used by `input`'s field/message labels (and, through it, `range_slider`'s
//!   and `wheel_picker`'s option labels): a control whose ink only changes on
//!   a rebuild.
//! - [`LabelRun`] — the run is shaped with a sentinel ink
//!   ([`SHAPING_INK`]) and every `GlyphRun` is re-brushed at paint time. Used
//!   by `button` (and, through it, `expanding_arrow_button`,
//!   `expandable_control`, `action_swap`), `animated_badge`, and each of the
//!   seven navigation components (`tabs`, `dock`, `animated_sidebar`,
//!   `bounce_sidebar`, `file_tree`, `bouncy_accordion`, `preview_rail`) — a
//!   control that recolors its text on hover, on a press, or across a state
//!   morph, all repaint-only changes a baked-ink run would otherwise force a
//!   relayout for.
//!
//! [`paint_glyph_run`] is the lower-level primitive both `paint` methods are
//! built on: it also covers `number`'s per-digit runs and
//! `text_animation`'s per-cell run, neither of which caches a whole shaped
//! string the way `Label`/`LabelRun` do (a rolling digit column and a
//! per-character cell each hold their own `TextLayout`s directly).
//!
//! [`label_style`] is `button`'s control-label style — medium weight in the
//! catalog's sans family — shared verbatim with its three siblings above. Its
//! family is the unthemed base; [`LabelRun::layout_themed`] swaps in the live
//! theme's.
//!
//! # Taking the font family from the theme
//!
//! A catalog text's family comes from the live theme's type scale, resolved at
//! layout, so a theme swap or `frust_testing::frame::pin_type_scale` restyles
//! it. Naming a family at build time does neither. The opt-in names a
//! [`ThemeTextType`] role, re-exported here, and differs only by where the text
//! is shaped:
//!
//! - a cached run ([`Label`]/[`LabelRun`]) calls
//!   [`layout_themed`](LabelRun::layout_themed)`(ctx, &style, role)` instead of
//!   `layout(ctx, &style)`;
//! - a widget shaping its own `TextLayout`s passes its style through
//!   [`themed_style`] with `Theme::from_layout_ctx(ctx)` in `layout`, and keys
//!   its shaped-run cache on the style that returns;
//! - a plain `frust::text(..)` child calls `.themed_family(role)`, the
//!   framework's own opt-in on the same enum.
//!
//! Only the family is taken from the role; size, weight, tracking and ink stay
//! the style's own. beUI's type scale ([`crate::tokens::type_scale`]) puts the
//! same Geist stack in every slot, so the role is a semantic choice (a control
//! label is `LabelLarge`, a caption `LabelSmall`, a heading a title role) and
//! never changes what the catalog's own theme renders. With no theme threaded
//! in, the family is [`crate::tokens::sans_family`], which is what that theme
//! would resolve.
//!
//! Plain `layout` with a family set in code is for text no role carries:
//! monospace (`TypeScale` has no mono slot, so [`crate::tokens::mono_family`]
//! stays explicit). A site that keeps an explicit family says why in a comment.

use frust::Theme;
use frust::TypeScale;
use frust::authoring::text::{FontFamily, FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{Brush, Color, LayoutCtx, PaintScene, Point, Size};

/// The type-scale role a catalog text takes its font family from. The
/// framework's own enum (`frust::authoring::ThemeTextType`), re-exported so a
/// component names it beside the runs it opts in.
pub(crate) use frust::authoring::ThemeTextType;

/// `role`'s slot in `scale`. `frust-widgets` keeps its own copy of this
/// mapping private, so the catalog restates it; the test
/// `every_role_reads_its_own_slot` pins every arm.
fn role_slot(scale: &TypeScale, role: ThemeTextType) -> &TextStyle {
    match role {
        ThemeTextType::DisplayLarge => &scale.display_large,
        ThemeTextType::DisplayMedium => &scale.display_medium,
        ThemeTextType::DisplaySmall => &scale.display_small,
        ThemeTextType::HeadlineLarge => &scale.headline_large,
        ThemeTextType::HeadlineMedium => &scale.headline_medium,
        ThemeTextType::HeadlineSmall => &scale.headline_small,
        ThemeTextType::TitleLarge => &scale.title_large,
        ThemeTextType::TitleMedium => &scale.title_medium,
        ThemeTextType::TitleSmall => &scale.title_small,
        ThemeTextType::BodyLarge => &scale.body_large,
        ThemeTextType::BodyMedium => &scale.body_medium,
        ThemeTextType::BodySmall => &scale.body_small,
        ThemeTextType::LabelLarge => &scale.label_large,
        ThemeTextType::LabelMedium => &scale.label_medium,
        ThemeTextType::LabelSmall => &scale.label_small,
        ThemeTextType::DisplayLargeEmphasized => &scale.display_large_emphasized,
        ThemeTextType::DisplayMediumEmphasized => &scale.display_medium_emphasized,
        ThemeTextType::DisplaySmallEmphasized => &scale.display_small_emphasized,
        ThemeTextType::HeadlineLargeEmphasized => &scale.headline_large_emphasized,
        ThemeTextType::HeadlineMediumEmphasized => &scale.headline_medium_emphasized,
        ThemeTextType::HeadlineSmallEmphasized => &scale.headline_small_emphasized,
        ThemeTextType::TitleLargeEmphasized => &scale.title_large_emphasized,
        ThemeTextType::TitleMediumEmphasized => &scale.title_medium_emphasized,
        ThemeTextType::TitleSmallEmphasized => &scale.title_small_emphasized,
        ThemeTextType::BodyLargeEmphasized => &scale.body_large_emphasized,
        ThemeTextType::BodyMediumEmphasized => &scale.body_medium_emphasized,
        ThemeTextType::BodySmallEmphasized => &scale.body_small_emphasized,
        ThemeTextType::LabelLargeEmphasized => &scale.label_large_emphasized,
        ThemeTextType::LabelMediumEmphasized => &scale.label_medium_emphasized,
        ThemeTextType::LabelSmallEmphasized => &scale.label_small_emphasized,
    }
}

/// The family text in `role` paints in: the live theme's `role` family, or
/// [`crate::tokens::sans_family`] with no theme (see the
/// [module docs](self)).
pub(crate) fn role_family(role: ThemeTextType, theme: Option<&Theme>) -> FontFamily {
    theme.map_or_else(crate::tokens::sans_family, |theme| {
        role_slot(&theme.type_scale, role).family.clone()
    })
}

/// `style` with its family replaced by [`role_family`]`(role, theme)`, and
/// every other field kept. Call it in `layout` with
/// `Theme::from_layout_ctx(ctx)` and shape with the result.
pub(crate) fn themed_style(
    style: TextStyle,
    role: ThemeTextType,
    theme: Option<&Theme>,
) -> TextStyle {
    TextStyle {
        family: role_family(role, theme),
        ..style
    }
}

/// The ink [`LabelRun`] shapes with. Never painted — keeping it constant keeps
/// the shape cache from missing when only the color changes.
pub(crate) const SHAPING_INK: Color = Color::BLACK;

/// A cached, lazily-shaped text run whose color is baked into the shaped
/// layout.
///
/// Shaping happens in [`layout`](Self::layout) (the only pass carrying a
/// [`TextContext`]) and the color is part of the style, so a color change
/// re-shapes. A run whose color animates should fade through a `push_layer`
/// alpha rather than through its style, or use [`LabelRun`] instead.
pub(crate) struct Label {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl Label {
    /// A run holding `content`, unshaped until the first [`Label::layout`].
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    /// Replace the content, invalidating the cached shape only if it actually
    /// changed. Reports whether anything changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) -> bool {
        let content = content.into();
        if self.content == content {
            return false;
        }
        self.content = content;
        self.layout = None;
        true
    }

    /// Shape (or reuse) the run in `style` and return its measured size.
    pub(crate) fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        size
    }

    /// [`Label::layout`], with the family taken from the live theme's `role`
    /// ([`themed_style`]). The cache compares the resolved style, so a theme
    /// swap that changes the family reshapes the run.
    pub(crate) fn layout_themed(
        &mut self,
        ctx: &mut LayoutCtx,
        style: &TextStyle,
        role: ThemeTextType,
    ) -> Size {
        let style = themed_style(style.clone(), role, Theme::from_layout_ctx(ctx));
        self.layout(ctx, &style)
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Emit this run's glyphs at absolute `origin`, in the ink it was shaped
    /// with. A no-op before the first [`Label::layout`].
    pub(crate) fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A cached, lazily-shaped text run whose color is applied at **paint** time.
///
/// The shape/measure half matches [`Label`]; the difference is
/// [`paint`](Self::paint), which re-brushes each `GlyphRun` instead of
/// painting the ink the layout was shaped with — so a hover recolor or a
/// state morph costs a repaint rather than a relayout. Shape the run with
/// [`SHAPING_INK`] to keep the cache key free of the color.
pub(crate) struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl LabelRun {
    /// A run holding `content`, unshaped until the first [`LabelRun::layout`].
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_style: None,
        }
    }

    /// Replace the content, invalidating the cached shape only if it actually
    /// changed. Reports whether anything changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) -> bool {
        let content = content.into();
        if self.content == content {
            return false;
        }
        self.content = content;
        self.layout = None;
        true
    }

    /// The text this run holds.
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    /// Shape (or reuse) the run in `style` and return its measured size.
    pub(crate) fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.shaped_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.shaped_style = Some(style.clone());
        size
    }

    /// [`LabelRun::layout`], with the family taken from the live theme's
    /// `role` ([`themed_style`]). The cache compares the resolved style, so a
    /// theme swap that changes the family reshapes the run.
    pub(crate) fn layout_themed(
        &mut self,
        ctx: &mut LayoutCtx,
        style: &TextStyle,
        role: ThemeTextType,
    ) -> Size {
        let style = themed_style(style.clone(), role, Theme::from_layout_ctx(ctx));
        self.layout(ctx, &style)
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, TextLayout::size)
    }

    /// Paint the run at `origin` in `color`, overriding whatever ink it was
    /// shaped with. A run that has never been laid out paints nothing.
    pub(crate) fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for mut run in layout.to_scene_runs(origin) {
                run.brush = Brush::Solid(color);
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// Emit an already-shaped `run`'s glyphs at `origin` under `brush`, overriding
/// whatever ink it was shaped with. A no-op when `run` is `None` — the shape
/// [`number`](super::components::number)'s digit columns and
/// [`text_animation`](super::components::text_animation)'s per-character
/// cells need, since each holds a `Vec` of bare [`TextLayout`]s rather than
/// one cached [`LabelRun`].
pub(crate) fn paint_glyph_run(
    run: Option<&TextLayout>,
    origin: Point,
    brush: &Brush,
    scene: &mut dyn PaintScene,
) {
    let Some(run) = run else { return };
    for mut glyphs in run.to_scene_runs(origin) {
        glyphs.brush = brush.clone();
        scene.draw_glyph_run(glyphs);
    }
}

/// The catalog's control-label text style at `size`, in beUI's own family and
/// the medium weight every control label upstream carries (`font-medium`).
/// Shared by `button` and its three siblings (`expanding_arrow_button`,
/// `expandable_control`, `action_swap`).
pub(crate) fn label_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::sans_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(size as f32, SHAPING_INK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use std::any::Any;

    /// Records the ink every glyph run is drawn with.
    #[derive(Default)]
    struct Inks(Vec<Color>);

    impl PaintScene for Inks {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.0.push(color);
            }
        }
    }

    /// A `LayoutCtx` carrying a real text context, the only resource either
    /// run needs to shape.
    fn with_text_ctx<R>(f: impl FnOnce(&mut LayoutCtx) -> R) -> R {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        f(&mut ctx)
    }

    #[test]
    fn a_label_shapes_once_and_paints_the_baked_ink() {
        let ink = Color::from_rgb8(0x11, 0x22, 0x33);
        let mut label = Label::new("Email");
        let measured = with_text_ctx(|ctx| label.layout(ctx, &label_style_at(14.0, ink)));
        assert!(measured.width > 0.0 && measured.height > 0.0);
        assert_eq!(label.size(), measured);

        let mut scene = Inks::default();
        label.paint(Point::ZERO, &mut scene);
        assert_eq!(scene.0, vec![ink]);

        assert!(label.set_content("Password"), "a real change re-shapes");
        assert!(!label.set_content("Password"), "an identical one does not");
    }

    #[test]
    fn a_label_run_shapes_once_and_repaints_in_any_ink() {
        let mut run = LabelRun::new("Ship");
        let style = label_style(14.0);
        let measured = with_text_ctx(|ctx| run.layout(ctx, &style));
        assert!(measured.width > 0.0);
        assert_eq!(run.size(), measured);
        assert_eq!(run.content(), "Ship");

        let ink = Color::from_rgb8(1, 2, 3);
        let mut rec = Inks::default();
        run.paint(Point::ORIGIN, ink, &mut rec);
        assert!(rec.0.iter().all(|c| *c == ink));

        run.set_content("Ship");
        assert_eq!(run.size(), measured, "an identical string re-uses the run");
    }

    #[test]
    fn an_unshaped_run_paints_nothing() {
        let mut scene = Inks::default();
        Label::new("Save").paint(Point::ZERO, &mut scene);
        LabelRun::new("Save").paint(Point::ZERO, Color::WHITE, &mut scene);
        assert!(scene.0.is_empty());
        assert_eq!(LabelRun::new("Save").size(), Size::ZERO);
    }

    #[test]
    fn paint_glyph_run_is_a_no_op_on_an_unshaped_run() {
        let mut scene = Inks::default();
        paint_glyph_run(None, Point::ZERO, &Brush::Solid(Color::WHITE), &mut scene);
        assert!(scene.0.is_empty());
    }

    /// A style for [`Label`], which bakes color into the shape (unlike
    /// [`label_style`], whose color is always [`SHAPING_INK`]).
    fn label_style_at(size: f32, color: Color) -> TextStyle {
        TextStyle {
            family: crate::tokens::sans_family(),
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(size, color)
        }
    }

    // ---- The family opt-in ------------------------------------------------

    use super::typeface_probe::{self, ALL_ROLES, Face};

    /// Lay a closure's runs out against `tcx` with `theme` threaded, the way
    /// the render root does.
    fn with_theme<R>(
        tcx: &mut TextContext,
        theme: Option<&Theme>,
        f: impl FnOnce(&mut LayoutCtx) -> R,
    ) -> R {
        let mut ctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        f(&mut ctx)
    }

    /// The faces a shaped run paints in.
    fn run_faces(run: &LabelRun) -> Vec<Face> {
        let mut rec = typeface_probe::FaceRecorder::default();
        run.paint(Point::ZERO, Color::BLACK, &mut rec);
        rec.faces
    }

    #[test]
    fn every_role_reads_its_own_slot() {
        // One distinct family per slot, so a mis-wired arm resolves a
        // neighbour's name instead of its own.
        let mut theme = crate::theme();
        let mut names = Vec::new();
        for (index, role) in ALL_ROLES.into_iter().enumerate() {
            let family = FontFamily::named(format!("Role Probe {index}"));
            typeface_probe::set_role_family(&mut theme, role, family.clone());
            names.push(family);
        }
        for (role, family) in ALL_ROLES.into_iter().zip(names) {
            assert_eq!(role_family(role, Some(&theme)), family, "{role:?}");
        }
    }

    #[test]
    fn without_a_theme_the_family_is_the_catalog_sans_stack() {
        for role in ALL_ROLES {
            assert_eq!(role_family(role, None), crate::tokens::sans_family());
        }
        // Only the family moves: the rest of the style is the caller's.
        let base = label_style(14.0);
        let themed = themed_style(base.clone(), ThemeTextType::LabelLarge, None);
        assert_eq!(themed, base);
    }

    #[test]
    fn themed_style_takes_only_the_family_from_the_role() {
        let mut theme = crate::theme();
        let probe = FontFamily::named("Label Large Probe");
        theme.type_scale.label_large.family = probe.clone();
        let base = label_style(13.0);
        let themed = themed_style(base.clone(), ThemeTextType::LabelLarge, Some(&theme));
        assert_eq!(themed.family, probe);
        assert_eq!(
            TextStyle {
                family: base.family.clone(),
                ..themed
            },
            base,
            "size, weight, ink and tracking stay the caller's"
        );
    }

    #[test]
    fn a_themed_run_paints_in_the_roles_face() {
        let mut tcx = typeface_probe::text_context();
        let mut run = LabelRun::new("Ship");
        let mut label = Label::new("Email");
        let beui = crate::theme();
        with_theme(&mut tcx, Some(&beui), |ctx| {
            run.layout_themed(ctx, &label_style(14.0), ThemeTextType::LabelLarge);
            label.layout_themed(ctx, &label_style(14.0), ThemeTextType::LabelLarge);
        });
        assert_eq!(run_faces(&run), vec![Face::Geist]);
        let mut rec = typeface_probe::FaceRecorder::default();
        label.paint(Point::ZERO, &mut rec);
        assert_eq!(rec.faces, vec![Face::Geist]);

        // Control: the same string with no family of its own paints the
        // platform's system face, not Geist — so the assertion above is the
        // role's doing, not the only face on the host.
        let mut plain = LabelRun::new("Ship");
        with_theme(&mut tcx, Some(&beui), |ctx| {
            plain.layout(ctx, &TextStyle::new(14.0, SHAPING_INK));
        });
        assert!(!run_faces(&plain).contains(&Face::Geist));
    }

    #[test]
    fn a_theme_swap_reshapes_a_themed_run_in_the_new_face() {
        let mut tcx = typeface_probe::text_context();
        let mut run = LabelRun::new("Ship");
        let mut label = Label::new("Email");
        let style = label_style(14.0);
        let beui = crate::theme();
        let mono = typeface_probe::mono_role_theme();
        let layout = |tcx: &mut TextContext, run: &mut LabelRun, label: &mut Label, theme| {
            with_theme(tcx, Some(theme), |ctx| {
                run.layout_themed(ctx, &style, ThemeTextType::LabelLarge);
                label.layout_themed(ctx, &style, ThemeTextType::LabelLarge);
            });
        };

        layout(&mut tcx, &mut run, &mut label, &beui);
        let settled = tcx.shape_cache_stats();
        layout(&mut tcx, &mut run, &mut label, &beui);
        assert_eq!(
            tcx.shape_cache_stats(),
            settled,
            "an unchanged theme reshapes nothing"
        );

        layout(&mut tcx, &mut run, &mut label, &mono);
        assert!(
            tcx.shape_cache_stats().shapes > settled.shapes,
            "a family swap must reshape, not serve the old face"
        );
        assert_eq!(run_faces(&run), vec![Face::GeistMono]);
        let mut rec = typeface_probe::FaceRecorder::default();
        label.paint(Point::ZERO, &mut rec);
        assert_eq!(rec.faces, vec![Face::GeistMono]);
    }

    #[test]
    fn plain_layout_keeps_the_family_set_in_code() {
        // The explicit path: a family set in code (the mono sites) is not
        // redirected by the theme.
        let mut tcx = typeface_probe::text_context();
        let mut run = LabelRun::new("0x1f");
        let style = TextStyle {
            family: crate::tokens::mono_family(),
            ..TextStyle::new(14.0, SHAPING_INK)
        };
        let mut sans_roles = crate::theme();
        for role in ALL_ROLES {
            typeface_probe::set_role_family(&mut sans_roles, role, crate::tokens::sans_family());
        }
        with_theme(&mut tcx, Some(&sans_roles), |ctx| run.layout(ctx, &style));
        assert_eq!(run_faces(&run), vec![Face::GeistMono]);
    }
}

/// A render-time typeface probe for the catalog's text: paints a view through
/// a real `RenderRoot` with both bundled faces registered and reports, per
/// painted glyph run, which face its font bytes are.
///
/// Registering Geist proves nothing about what paints — a run that never asks
/// for the theme's family paints the platform's system face beside a
/// registered Geist — so the component tests assert on the runs.
///
/// [`assert_paints_only_in_geist`] pairs its identity check with a control: a
/// plain `frust::text(..)` (the system-UI family) painted the same way must
/// have no Geist run, or on this host an un-opted run would be
/// indistinguishable from an opted-in one. [`assert_follows_a_live_family_swap`]
/// is the discriminating half for text the beUI theme renders in Geist either
/// way: on the same retained tree it swaps to a theme whose every role is Geist
/// Mono and requires every run to follow, then swaps back.
#[cfg(test)]
pub(crate) mod typeface_probe {
    use std::any::Any;

    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::{FontFamily, TextContext};
    use frust::authoring::{Color, InputEvent, PaintScene, Point, Size, View};
    use frust::{FrameTime, Theme};
    use frust_core::RenderRoot;

    use super::ThemeTextType;
    use crate::tokens::fonts::{GEIST_MONO_VARIABLE_INDEX, GEIST_VARIABLE_INDEX};

    /// Which bundled face a painted run shaped against.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Face {
        Geist,
        GeistMono,
        /// Anything else — the platform's system face, in practice.
        Other,
    }

    /// Classify `bytes` against the two bundled faces.
    fn face_of(bytes: &[u8]) -> Face {
        let data = crate::tokens::font_data();
        if bytes == data[GEIST_VARIABLE_INDEX] {
            Face::Geist
        } else if bytes == data[GEIST_MONO_VARIABLE_INDEX] {
            Face::GeistMono
        } else {
            Face::Other
        }
    }

    /// Records the face of every glyph run painted into it.
    #[derive(Default)]
    pub(crate) struct FaceRecorder {
        pub(crate) faces: Vec<Face>,
    }

    impl PaintScene for FaceRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.faces.push(face_of(run.font.font().data.as_ref()));
        }
    }

    /// A text context with both bundled faces registered, as `install()`
    /// leaves a shell's.
    pub(crate) fn text_context() -> TextContext {
        let mut tcx = TextContext::new();
        for bytes in crate::tokens::font_data() {
            tcx.register_fonts(bytes.to_vec())
                .expect("the bundled beUI faces register");
        }
        tcx
    }

    /// Every role, in `ThemeTextType`'s declaration order.
    pub(crate) const ALL_ROLES: [ThemeTextType; 30] = [
        ThemeTextType::DisplayLarge,
        ThemeTextType::DisplayMedium,
        ThemeTextType::DisplaySmall,
        ThemeTextType::HeadlineLarge,
        ThemeTextType::HeadlineMedium,
        ThemeTextType::HeadlineSmall,
        ThemeTextType::TitleLarge,
        ThemeTextType::TitleMedium,
        ThemeTextType::TitleSmall,
        ThemeTextType::BodyLarge,
        ThemeTextType::BodyMedium,
        ThemeTextType::BodySmall,
        ThemeTextType::LabelLarge,
        ThemeTextType::LabelMedium,
        ThemeTextType::LabelSmall,
        ThemeTextType::DisplayLargeEmphasized,
        ThemeTextType::DisplayMediumEmphasized,
        ThemeTextType::DisplaySmallEmphasized,
        ThemeTextType::HeadlineLargeEmphasized,
        ThemeTextType::HeadlineMediumEmphasized,
        ThemeTextType::HeadlineSmallEmphasized,
        ThemeTextType::TitleLargeEmphasized,
        ThemeTextType::TitleMediumEmphasized,
        ThemeTextType::TitleSmallEmphasized,
        ThemeTextType::BodyLargeEmphasized,
        ThemeTextType::BodyMediumEmphasized,
        ThemeTextType::BodySmallEmphasized,
        ThemeTextType::LabelLargeEmphasized,
        ThemeTextType::LabelMediumEmphasized,
        ThemeTextType::LabelSmallEmphasized,
    ];

    /// Set `role`'s family in `theme`'s type scale.
    pub(crate) fn set_role_family(theme: &mut Theme, role: ThemeTextType, family: FontFamily) {
        let scale = &mut theme.type_scale;
        let slot = match role {
            ThemeTextType::DisplayLarge => &mut scale.display_large,
            ThemeTextType::DisplayMedium => &mut scale.display_medium,
            ThemeTextType::DisplaySmall => &mut scale.display_small,
            ThemeTextType::HeadlineLarge => &mut scale.headline_large,
            ThemeTextType::HeadlineMedium => &mut scale.headline_medium,
            ThemeTextType::HeadlineSmall => &mut scale.headline_small,
            ThemeTextType::TitleLarge => &mut scale.title_large,
            ThemeTextType::TitleMedium => &mut scale.title_medium,
            ThemeTextType::TitleSmall => &mut scale.title_small,
            ThemeTextType::BodyLarge => &mut scale.body_large,
            ThemeTextType::BodyMedium => &mut scale.body_medium,
            ThemeTextType::BodySmall => &mut scale.body_small,
            ThemeTextType::LabelLarge => &mut scale.label_large,
            ThemeTextType::LabelMedium => &mut scale.label_medium,
            ThemeTextType::LabelSmall => &mut scale.label_small,
            ThemeTextType::DisplayLargeEmphasized => &mut scale.display_large_emphasized,
            ThemeTextType::DisplayMediumEmphasized => &mut scale.display_medium_emphasized,
            ThemeTextType::DisplaySmallEmphasized => &mut scale.display_small_emphasized,
            ThemeTextType::HeadlineLargeEmphasized => &mut scale.headline_large_emphasized,
            ThemeTextType::HeadlineMediumEmphasized => &mut scale.headline_medium_emphasized,
            ThemeTextType::HeadlineSmallEmphasized => &mut scale.headline_small_emphasized,
            ThemeTextType::TitleLargeEmphasized => &mut scale.title_large_emphasized,
            ThemeTextType::TitleMediumEmphasized => &mut scale.title_medium_emphasized,
            ThemeTextType::TitleSmallEmphasized => &mut scale.title_small_emphasized,
            ThemeTextType::BodyLargeEmphasized => &mut scale.body_large_emphasized,
            ThemeTextType::BodyMediumEmphasized => &mut scale.body_medium_emphasized,
            ThemeTextType::BodySmallEmphasized => &mut scale.body_small_emphasized,
            ThemeTextType::LabelLargeEmphasized => &mut scale.label_large_emphasized,
            ThemeTextType::LabelMediumEmphasized => &mut scale.label_medium_emphasized,
            ThemeTextType::LabelSmallEmphasized => &mut scale.label_small_emphasized,
        };
        slot.family = family;
    }

    /// beUI's own theme with every type-scale role's family swapped to the
    /// Geist Mono stack — what a font picker rewriting the scale pushes.
    pub(crate) fn mono_role_theme() -> Theme {
        let mut theme = crate::theme();
        for role in ALL_ROLES {
            set_role_family(&mut theme, role, crate::tokens::mono_family());
        }
        theme
    }

    /// A render root holding one view, laid out and painted in `window`
    /// against [`text_context`].
    pub(crate) struct Probe<V: View<()>, F: FnMut(&mut ()) -> V> {
        root: RenderRoot<(), V>,
        tcx: TextContext,
        logic: F,
        window: Size,
        clock_ms: u64,
    }

    impl<V: View<()> + 'static, F: FnMut(&mut ()) -> V> Probe<V, F> {
        /// Build `logic`'s view under `theme`.
        pub(crate) fn new(logic: F, window: Size, theme: Theme) -> Self {
            let mut probe = Probe {
                root: RenderRoot::new(),
                tcx: text_context(),
                logic,
                window,
                clock_ms: 0,
            };
            probe.root.set_theme(Box::new(theme));
            probe.root.rebuild(&mut probe.logic, &mut ());
            probe
        }

        /// Replace the theme on the retained tree, with no view change.
        pub(crate) fn swap_theme(&mut self, theme: Theme) {
            self.root.set_theme(Box::new(theme));
        }

        /// Dispatch `event` into the tree — for a component that opens on
        /// input rather than on a builder flag.
        pub(crate) fn event(&mut self, event: &InputEvent) {
            self.root.event(&mut (), event);
        }

        /// Paint once more into `scene` at the probe's current clock — to read
        /// geometry a pointer target is computed from.
        pub(crate) fn paint_into(&mut self, scene: &mut dyn PaintScene) {
            self.root
                .paint(scene, FrameTime::from_nanos(self.clock_ms * 1_000_000));
        }

        /// Lay out and paint twice — once to latch any entrance clock, once
        /// five seconds on when it has settled — and return the faces of the
        /// settled paint's glyph runs.
        pub(crate) fn frame(&mut self) -> Vec<Face> {
            self.root
                .layout_with_text(self.window, &mut self.tcx as &mut dyn Any);
            self.root.paint(
                &mut FaceRecorder::default(),
                FrameTime::from_nanos(self.clock_ms * 1_000_000),
            );
            self.clock_ms += 5_000;
            self.root
                .layout_with_text(self.window, &mut self.tcx as &mut dyn Any);
            let mut rec = FaceRecorder::default();
            self.root
                .paint(&mut rec, FrameTime::from_nanos(self.clock_ms * 1_000_000));
            self.clock_ms += 5_000;
            rec.faces
        }
    }

    /// Asserts `faces` is non-empty and every entry is `face`.
    #[track_caller]
    pub(crate) fn assert_all(what: &str, when: &str, faces: &[Face], face: Face) {
        assert!(!faces.is_empty(), "{what} painted no glyph run {when}");
        let foreign = faces.iter().filter(|f| **f != face).count();
        assert_eq!(
            foreign,
            0,
            "{foreign} of {} glyph run(s) in {what} {when} were not {face:?} ({faces:?}) — \
             the text is not taking its family from the theme's type scale",
            faces.len()
        );
    }

    /// The control: a plain `frust::text(..)` — the system-UI family — painted
    /// under beUI's theme in `window` has no Geist run.
    #[track_caller]
    pub(crate) fn assert_control(what: &str, window: Size) {
        let control = Probe::new(|_: &mut ()| frust::text("Hello"), window, crate::theme()).frame();
        assert!(
            !control.contains(&Face::Geist),
            "control: a plain text() shaped in Geist on this host ({control:?}), so the \
             identity check for {what} would prove nothing"
        );
    }

    /// Paints `logic`'s view under beUI's own theme and asserts every glyph
    /// run shaped in Geist, after [`assert_control`].
    #[track_caller]
    pub(crate) fn assert_paints_only_in_geist<V: View<()> + 'static>(
        what: &str,
        logic: impl FnMut(&mut ()) -> V,
        window: Size,
    ) {
        assert_control(what, window);
        let faces = Probe::new(logic, window, crate::theme()).frame();
        assert_all(what, "under the beUI theme", &faces, Face::Geist);
    }

    /// [`assert_follows_a_live_family_swap_on`] a fresh probe of `logic`'s
    /// view under beUI's theme.
    #[track_caller]
    pub(crate) fn assert_follows_a_live_family_swap<V: View<()> + 'static>(
        what: &str,
        logic: impl FnMut(&mut ()) -> V,
        window: Size,
    ) {
        let mut probe = Probe::new(logic, window, crate::theme());
        assert_follows_a_live_family_swap_on(what, &mut probe);
    }

    /// On `probe`'s retained tree, currently under beUI's theme: every run is
    /// Geist, then Geist Mono after a swap to [`mono_role_theme`], then Geist
    /// again after swapping back.
    #[track_caller]
    pub(crate) fn assert_follows_a_live_family_swap_on<
        V: View<()> + 'static,
        F: FnMut(&mut ()) -> V,
    >(
        what: &str,
        probe: &mut Probe<V, F>,
    ) {
        assert_all(what, "under the beUI theme", &probe.frame(), Face::Geist);
        probe.swap_theme(mono_role_theme());
        assert_all(
            what,
            "after a swap to Geist Mono roles",
            &probe.frame(),
            Face::GeistMono,
        );
        probe.swap_theme(crate::theme());
        assert_all(what, "after swapping back", &probe.frame(), Face::Geist);
    }
}
