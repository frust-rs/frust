//! The catalog's two retained text runs, and [`themed_family`], the opt-in
//! that takes their font family from the live theme.
//!
//! A shadcn component that draws leaf text shapes its own run rather than
//! nesting a `frust::text` child: the framework's baseline text widget bakes its
//! color into the layout, and a component wants either a color of its own
//! choosing ([`Label`]) or a color that changes on hover without a relayout
//! ([`LabelRun`]). Both cache the shaped layout and re-shape only when the text
//! or the [`TextStyle`] it was shaped with actually changes.
//!
//! # Which one a component wants
//!
//! - [`Label`] — the color is part of the style, baked in at layout time. The
//!   right choice when the ink only changes on a rebuild (a variant swap, a
//!   disabled flag): `badge`, `button`, `kbd`, `label`, `bubble`.
//! - [`LabelRun`] — the run is shaped with a sentinel ink and every `GlyphRun`
//!   is re-brushed at paint time. The right choice when the ink depends on
//!   something that only ever requests a *repaint* (hover, a selected item),
//!   since a baked color would otherwise lag a relayout behind: `tabs`,
//!   `toggle`, `toggle_group`, `breadcrumb`.
//!
//! # The font family comes from the live theme
//!
//! A catalog text names its family through a type-scale role, resolved at
//! layout, so a theme swap or `frust_testing::frame::pin_type_scale` can
//! redirect it (`docs/WIDGETS_CODE_STANDARDS.md`'s theming rules). A
//! `frust::text` child opts in with `.themed_family(role)`. A run shaped here
//! opts in by building its style through [`themed_family`] from
//! `Theme::from_layout_ctx` in `layout`. Both caches key on the whole style, so
//! a swapped family reshapes instead of serving the old face.
//!
//! Every slot of shadcn's own type scale carries the same Inter stack
//! ([`type_scale`](crate::tokens::type_scale)), so under shadcn's theme the
//! role a site names changes nothing it paints. The role matters only under a
//! theme whose roles differ, so it is picked by what the text is and its
//! nearest size: `Title*` for a heading, `Body*` for running or description
//! text, `Label*` for a control's own label. At `text-xs` that is `BodySmall`
//! or `LabelMedium`. At `text-sm` it is `BodyMedium`, `LabelLarge` or
//! `TitleSmall`. At `text-base` and up it is `BodyLarge` or `TitleMedium`.
//! Mono text has no role, because `TypeScale` has no monospace slot. It keeps
//! [`mono_family`](crate::tokens::mono_family) explicitly and follows no theme.

use frust::Theme;
use frust::authoring::{
    Brush, Color, LayoutCtx, PaintScene, Point, Size, ThemeTextType,
    text::{TextContext, TextLayout, TextStyle},
};

/// `style` with its font family replaced by the live theme's `role` family,
/// the opt-in for a run a component shapes itself. This is the counterpart of a
/// `frust::text` child's `.themed_family(role)`.
///
/// Only the family is taken from the role. Size, weight, line height, tracking
/// and color stay as `style` sets them. With no theme, `style` is returned
/// unchanged, so its own family is the unthemed fallback. Call it in `layout`
/// with `Theme::from_layout_ctx(ctx)` and shape the result with
/// [`Label::layout`]/[`LabelRun::layout`]. Their caches key on the whole
/// style, so a theme swap that changes the family reshapes the run.
pub(crate) fn themed_family(
    mut style: TextStyle,
    theme: Option<&Theme>,
    role: ThemeTextType,
) -> TextStyle {
    if let Some(theme) = theme {
        style.family = role.style_in(&theme.type_scale).family.clone();
    }
    style
}

/// The ink a [`LabelRun`] is *shaped* with. Never painted: every run is
/// re-brushed with the state's resolved color, and keeping the shaping color
/// constant keeps the shape cache from missing on a color change.
pub(crate) const SHAPING_INK: Color = Color::BLACK;

/// A cached, lazily-shaped text run whose color is baked into the shaped
/// layout.
pub(crate) struct Label {
    content: String,
    layout: Option<TextLayout>,
    shaped_style: Option<TextStyle>,
}

impl Label {
    /// A run holding `content`, unshaped until the first [`layout`](Self::layout).
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            shaped_style: None,
        }
    }

    /// Replace the text, dropping the cached layout only if it actually changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    /// The text this run holds — the accessible name a component's `semantics`
    /// reports, without retaining a second copy of the string.
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
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

    /// Paint the run at `origin` in the color it was shaped with. A run that has
    /// never been laid out paints nothing.
    pub(crate) fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A cached, lazily-shaped text run whose color is applied at paint time.
///
/// The shape/measure half matches [`Label`]; the difference is
/// [`paint`](Self::paint), which re-brushes each `GlyphRun` instead of painting
/// the ink the layout was shaped with — so a hover recolor costs a repaint
/// rather than a relayout. Shape the run with [`SHAPING_INK`] to keep the cache
/// key free of the color.
pub(crate) struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl LabelRun {
    /// A run holding `content`, unshaped until the first [`layout`](Self::layout).
    pub(crate) fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    /// Replace the text, dropping the cached layout only if it actually changed.
    pub(crate) fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
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

    /// The text this run holds — the accessible name a component's `semantics`
    /// reports, without retaining a second copy of the string.
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    /// The measured size of the last shaped run (`ZERO` before the first
    /// layout).
    pub(crate) fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
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

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::{FontFamily, FontWeight};
    use frust::authoring::{BoxConstraints, scene::GlyphRun};
    use std::any::Any;

    #[test]
    fn themed_family_takes_only_the_roles_family_and_only_under_a_theme() {
        let base = TextStyle {
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(13.0, Color::from_rgb8(0x11, 0x22, 0x33))
        };
        assert_eq!(
            themed_family(base.clone(), None, ThemeTextType::BodyMedium),
            base,
            "unthemed, the style's own family is the fallback"
        );

        let mut theme = crate::theme();
        let probe = FontFamily::named("Role Probe");
        theme.type_scale.body_medium.family = probe.clone();
        assert_eq!(
            themed_family(base.clone(), Some(&theme), ThemeTextType::BodyMedium),
            TextStyle {
                family: probe,
                ..base
            },
            "themed, only the family changes: size, weight and color stay the style's own"
        );
    }

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

    /// A `LayoutCtx` carrying a real text context, the only resource either run
    /// needs to shape.
    fn with_text_ctx<R>(f: impl FnOnce(&mut LayoutCtx) -> R) -> R {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), None);
        f(&mut ctx)
    }

    fn style(size: f32, ink: Color) -> TextStyle {
        TextStyle::new(size, ink)
    }

    #[test]
    fn a_label_measures_its_text_and_paints_the_baked_ink() {
        let ink = Color::from_rgb8(0x11, 0x22, 0x33);
        let mut label = Label::new("Save");
        let size = with_text_ctx(|ctx| label.layout(ctx, &style(14.0, ink)));
        assert!(size.width > 0.0 && size.height > 0.0);

        let mut scene = Inks::default();
        label.paint(Point::ZERO, &mut scene);
        assert_eq!(scene.0, vec![ink]);
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
    fn set_content_invalidates_the_cache_only_on_a_real_change() {
        let s = style(14.0, SHAPING_INK);
        let mut run = LabelRun::new("Tab");
        let first = with_text_ctx(|ctx| run.layout(ctx, &s));

        run.set_content("Tab");
        assert!(run.layout.is_some(), "an identical string keeps the layout");
        run.set_content("A much longer tab label");
        assert!(run.layout.is_none(), "a changed string drops it");

        let second = with_text_ctx(|ctx| run.layout(ctx, &s));
        assert!(second.width > first.width);
    }

    /// The reason [`LabelRun`] exists: recoloring is a paint-time operation, so
    /// the shaped layout must survive it untouched.
    #[test]
    fn recoloring_a_run_reuses_the_shaped_layout() {
        let s = style(14.0, SHAPING_INK);
        let mut run = LabelRun::new("Bold");
        let shaped = with_text_ctx(|ctx| run.layout(ctx, &s));

        for ink in [Color::WHITE, Color::from_rgb8(0xE7, 0x00, 0x0B)] {
            let mut scene = Inks::default();
            run.paint(Point::ZERO, ink, &mut scene);
            assert_eq!(scene.0, vec![ink], "the shaping ink is never painted");
        }
        // Re-laying out in the same style is a cache hit: same size, and the
        // measurement never went back through shaping.
        assert_eq!(with_text_ctx(|ctx| run.layout(ctx, &s)), shaped);
        assert_eq!(run.size(), shaped);
    }

    #[test]
    fn a_style_change_reshapes_but_an_identical_style_does_not() {
        let mut label = Label::new("Save");
        let small = with_text_ctx(|ctx| label.layout(ctx, &style(12.0, SHAPING_INK)));
        let large = with_text_ctx(|ctx| label.layout(ctx, &style(24.0, SHAPING_INK)));
        assert!(large.width > small.width);
        assert_eq!(
            with_text_ctx(|ctx| label.layout(ctx, &style(24.0, SHAPING_INK))),
            large
        );
        // A color-only change is still a style change for a baked-ink `Label`.
        let recolored =
            with_text_ctx(|ctx| label.layout(ctx, &style(24.0, Color::from_rgb8(1, 2, 3))));
        assert_eq!(recolored, large, "same metrics, freshly shaped");

        // Sanity: the sizes are usable as layout input.
        let bc = BoxConstraints::new(Size::ZERO, Size::new(400.0, 400.0));
        assert_eq!(bc.constrain(large), large);
    }
}

/// A render-time typeface probe for the catalog's text. It paints a view
/// through a real `RenderRoot` under a theme, with both bundled faces
/// registered the way [`install`](crate::install) registers them, and names
/// the face each painted glyph run shaped against.
///
/// Registering a face proves nothing about what paints: a run that never asks
/// for the theme's family paints the platform's system font right beside a
/// registered Inter. So these checks assert on the painted runs.
///
/// Every check is paired with a control. The control paints a plain
/// `frust::text(..)` through the same theme and faces, the system-UI request an
/// un-opted text makes, and requires that none of its runs is the expected
/// face. On a host where that request resolved to the expected face, the
/// identity check could not tell an opted-in text from an un-opted one, so the
/// control fails first.
#[cfg(all(test, feature = "bundled-fonts"))]
pub(crate) mod typeface_probe {
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::{FontFamily, TextContext};
    use frust::authoring::{Color, PaintScene, Point, Size, View};
    use frust::{FrameTime, Theme};
    use frust_core::RenderRoot;
    use std::any::Any;

    use crate::tokens::fonts::{INTER_VARIABLE_INDEX, JETBRAINS_MONO_VARIABLE_INDEX};
    use crate::tokens::{font_data, mono_family};

    /// The window every probe lays its view out in.
    const WINDOW: Size = Size::new(480.0, 360.0);

    /// The face a painted glyph run shaped against.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Face {
        Inter,
        JetBrainsMono,
        /// Neither bundled face: a platform or fallback font.
        Other,
    }

    impl Face {
        fn of(run: &GlyphRun) -> Self {
            let bytes = run.font.font().data.as_ref();
            if bytes == font_data()[INTER_VARIABLE_INDEX] {
                Self::Inter
            } else if bytes == font_data()[JETBRAINS_MONO_VARIABLE_INDEX] {
                Self::JetBrainsMono
            } else {
                Self::Other
            }
        }
    }

    /// Records the face of every painted glyph run.
    #[derive(Default)]
    struct FaceRecorder(Vec<Face>);

    impl PaintScene for FaceRecorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.0.push(Face::of(&run));
        }
    }

    /// A root over one view under one theme, with both bundled faces
    /// registered.
    struct Probe<V: View<()>> {
        root: RenderRoot<(), V>,
        tcx: TextContext,
    }

    impl<V: View<()>> Probe<V> {
        fn new(mut logic: impl FnMut(&mut ()) -> V, theme: Theme) -> Self {
            let mut tcx = TextContext::new();
            for face in font_data() {
                tcx.register_fonts(face.to_vec())
                    .expect("a bundled shadcn face registers");
            }
            let mut root = RenderRoot::new();
            root.set_theme(Box::new(theme));
            root.rebuild(&mut logic, &mut ());
            Self { root, tcx }
        }

        /// Lay the tree out, paint it, and return each glyph run's face.
        fn paint(&mut self) -> Vec<Face> {
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut recorder = FaceRecorder::default();
            self.root.paint(&mut recorder, FrameTime::ZERO);
            recorder.0
        }
    }

    /// shadcn's theme with all 30 type-scale roles naming `family`, the
    /// rewrite a font picker pushes.
    fn theme_with_every_role(family: &FontFamily) -> Theme {
        let mut theme = crate::theme();
        macro_rules! every_role {
            ($($role:ident),+ $(,)?) => {
                $( theme.type_scale.$role.family = family.clone(); )+
            };
        }
        every_role!(
            display_large,
            display_medium,
            display_small,
            headline_large,
            headline_medium,
            headline_small,
            title_large,
            title_medium,
            title_small,
            body_large,
            body_medium,
            body_small,
            label_large,
            label_medium,
            label_small,
            display_large_emphasized,
            display_medium_emphasized,
            display_small_emphasized,
            headline_large_emphasized,
            headline_medium_emphasized,
            headline_small_emphasized,
            title_large_emphasized,
            title_medium_emphasized,
            title_small_emphasized,
            body_large_emphasized,
            body_medium_emphasized,
            body_small_emphasized,
            label_large_emphasized,
            label_medium_emphasized,
            label_small_emphasized,
        );
        theme
    }

    /// The control (see the [module docs](self)): a plain `frust::text(..)`
    /// under `theme` paints no run in `expected`.
    #[track_caller]
    fn assert_unopted_text_misses(theme: &Theme, expected: Face) {
        let control = Probe::new(|_: &mut ()| frust::text("Hello"), theme.clone()).paint();
        let leaked = control.iter().filter(|face| **face == expected).count();
        assert_eq!(
            leaked,
            0,
            "control: {leaked} of {} run(s) of a plain text() that never opts in \
             shaped in {expected:?}, so on this host the identity check below \
             could not tell an opted-in text from an un-opted one",
            control.len(),
        );
    }

    /// `runs` is non-empty and every run is `expected`.
    #[track_caller]
    fn assert_every_run(what: &str, runs: &[Face], expected: Face, when: &str) {
        assert!(!runs.is_empty(), "{what} painted no glyph run {when}");
        let foreign: Vec<&Face> = runs.iter().filter(|face| **face != expected).collect();
        assert!(
            foreign.is_empty(),
            "{} of {} glyph run(s) in {what} {when} shaped in {foreign:?}, not \
             {expected:?}: the text is not taking the theme's type-scale family",
            foreign.len(),
            runs.len(),
        );
    }

    /// Paint `logic`'s view under shadcn's own theme and assert it painted at
    /// least one glyph run, every one in Inter, the family every slot of that
    /// theme's type scale names.
    #[track_caller]
    pub(crate) fn assert_paints_in_the_theme_face<V: View<()>>(
        what: &str,
        logic: impl FnMut(&mut ()) -> V,
    ) {
        let theme = crate::theme();
        assert_unopted_text_misses(&theme, Face::Inter);
        let runs = Probe::new(logic, theme).paint();
        assert_every_run(what, &runs, Face::Inter, "under shadcn's theme");
    }

    /// Assert `logic`'s view follows a live theme swap on one root. It first
    /// paints in Inter under shadcn's theme. After `set_theme` to a theme whose
    /// every role names JetBrains Mono, the next layout and paint must be in
    /// JetBrains Mono. A run whose family was fixed at build, or whose cache
    /// ignores the resolved family, keeps painting Inter and fails.
    #[track_caller]
    pub(crate) fn assert_follows_a_live_theme_swap<V: View<()>>(
        what: &str,
        logic: impl FnMut(&mut ()) -> V,
    ) {
        let swapped = theme_with_every_role(&mono_family());
        assert_unopted_text_misses(&swapped, Face::JetBrainsMono);

        let mut probe = Probe::new(logic, crate::theme());
        let before = probe.paint();
        assert_every_run(what, &before, Face::Inter, "before the swap");

        probe.root.set_theme(Box::new(swapped));
        let after = probe.paint();
        assert_every_run(what, &after, Face::JetBrainsMono, "after the swap");
    }
}
