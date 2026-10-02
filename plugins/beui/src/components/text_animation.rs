//! Ports beUI's `text-animation` component family (several upstream variants).
//!
//! **Source:** beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01. The registry entry `text-animation` names one file plus four
//! `extraFiles` and lists five examples; all five arrive here as
//! [`TextAnimationVariant`] arms of one component rather than five components:
//!
//! | upstream example | file | arm |
//! |---|---|---|
//! | `reveal` | `components/motion/text-reveal.tsx` | [`TextAnimationVariant::Reveal`] |
//! | `cascade` | `components/motion/text-cascade.tsx` | [`TextAnimationVariant::Cascade`] |
//! | `text-scramble` | `components/motion/text-scramble.tsx` | [`TextAnimationVariant::Scramble`] |
//! | `shimmer` | `components/motion/text-shimmer.tsx` | [`TextAnimationVariant::Shimmer`] |
//! | `chromatic-reveal` | `components/motion/chromatic-text-reveal.tsx` | [`TextAnimationVariant::Chromatic`] |
//!
//! # Two rendering routes, one component
//!
//! The five effects split cleanly in two, and this component carries both:
//!
//! - **Per-letter** ([`Reveal`](TextAnimationVariant::Reveal),
//!   [`Cascade`](TextAnimationVariant::Cascade)) — each grapheme is its own
//!   animatable cell, which is exactly [`crate::motion::chars`]. The letters are
//!   a [`CharCellsView`] child; this component only chooses the stagger and the
//!   per-cell staging.
//! - **Whole-run** ([`Scramble`](TextAnimationVariant::Scramble),
//!   [`Shimmer`](TextAnimationVariant::Shimmer),
//!   [`Chromatic`](TextAnimationVariant::Chromatic)) — the string is shaped as
//!   one run and the effect is applied to the *paint* of that run (a swapped
//!   string, a gradient brush). Splitting these into cells would throw away
//!   kerning for no gain, since none of them moves a letter independently.
//!
//! # The per-letter cell budget
//!
//! [`CharCells`](crate::motion::CharCells) itself puts no cap on how many cells
//! a string splits into, but this component does: [`TEXT_ANIMATION_CELL_BUDGET`]
//! graphemes (~80 — upstream's own assumed use: a heading, a caption, a status
//! line). Past that, a per-letter variant ([`Reveal`](TextAnimationVariant::Reveal),
//! [`Cascade`](TextAnimationVariant::Cascade)) falls back to painting the string
//! as one plain, non-animating shaped run rather than building one independently
//! animated, independently composited leaf per character — a debug build notes
//! the fallback on `stderr`. The whole-run variants have no such ceiling (one
//! shaped run whatever the length), but [`Scramble`](TextAnimationVariant::Scramble)
//! re-shapes on a tick and should still be kept short for its own sake.
//!
//! # Degradations against upstream
//!
//! - **No blur anywhere.** `text-reveal`'s `filter: blur(12px)` entrance and
//!   `chromatic-text-reveal`'s `blur(6px)` settle have no frust equivalent (the
//!   scene has no blur primitive a widget can reach); both degrade to the
//!   opacity and translation halves of the same transition.
//! - **Reveal's spring is substituted.** Upstream authors a per-component
//!   `{ stiffness: 140, damping: 26, mass: 1.2 }`, which is not one of
//!   [`crate::tokens::motion`]'s six. The nearest settling catalog spring,
//!   [`SPRING_LAYOUT`], drives the rise instead.
//! - **Reveal is character-split only.** Upstream's `split="word"` default
//!   animates whole words; this port always splits per grapheme, because that is
//!   what the substrate provides and the two look the same at a glance for the
//!   short strings the effect is for.
//! - **Chromatic is one string, not a prefix plus a cycling word list**, and its
//!   edge colors are beUI's own brand hues rather than upstream's five Tailwind
//!   hex literals (the catalog reads colors from tokens only).
//! - **Shimmer's sweep direction** is left-to-right; upstream animates a
//!   `background-position` pair whose visual direction depends on the resolved
//!   background box, which has no meaning outside CSS.

use std::time::Duration;

use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx, TickClass, View, Widget, any, build_child,
    rebuild_child, teardown_child, visit_children,
};
use frust::{Brightness, FrameTime, Theme};
use kurbo::{Point, Size};
use peniko::{Brush, Color, ColorStop, Gradient};

use crate::motion::chars::CHAR_CELLS_ROLE;
use crate::motion::{CellEffect, CharCells, CharCellsView, Ramp, Stagger, char_cascade};
use crate::style::{TEXT_BASE, with_alpha};
use crate::text::paint_glyph_run as paint_run;
use crate::text::themed_style;
use crate::tokens::motion::{EASE_IN_OUT, SPRING_LAYOUT};
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The per-letter cell budget, in graphemes — see the [module docs](self).
/// Past this many graphemes a per-letter variant
/// ([`Reveal`](TextAnimationVariant::Reveal), [`Cascade`](TextAnimationVariant::Cascade))
/// falls back to a plain, non-animating shaped run instead of one animatable
/// cell per character. **Community-approximate**: upstream states no exact
/// number, only its own assumed use (a heading, a caption, a status line);
/// ~80 is this port's own conservative reading of that use.
pub const TEXT_ANIMATION_CELL_BUDGET: usize = 80;

/// The number of grapheme cells `content` would split into — the count
/// [`TEXT_ANIMATION_CELL_BUDGET`] is measured against.
fn grapheme_count(content: &str) -> usize {
    CharCells::split(content).len()
}

/// Whether `cells` graphemes of `variant` exceed the budget — the one
/// predicate the plain-run fallback and its debug notice are both keyed on.
fn over_budget(variant: TextAnimationVariant, cells: usize) -> bool {
    variant.is_per_letter() && cells > TEXT_ANIMATION_CELL_BUDGET
}

/// Debug-only notice that a per-letter variant fell back to a plain run.
/// Raised by the retained [`TextAnimationWidget`] — once when it is built
/// over an over-budget `(content, variant)` and again only when a rebuild
/// changes those inputs — so a redundant rebuild never repeats it. Compiled
/// out of release builds entirely.
#[cfg(debug_assertions)]
fn note_over_budget(variant: TextAnimationVariant, cells: usize) {
    if over_budget(variant, cells) {
        eprintln!(
            "frust-beui: text_animation {variant:?} content is {cells} graphemes, past the \
             {TEXT_ANIMATION_CELL_BUDGET}-grapheme cell budget — falling back to a plain run \
             instead of one cell per character",
        );
    }
}

#[cfg(not(debug_assertions))]
fn note_over_budget(_variant: TextAnimationVariant, _cells: usize) {}

/// Per-letter delay of the reveal — `stagger = 0.09` (`text-reveal.tsx`).
const REVEAL_STAGGER: Duration = Duration::from_millis(90);

/// How far a revealing letter rises, as a fraction of its own height —
/// `yOffset = "40%"` (`text-reveal.tsx`).
const REVEAL_TRAVEL: f64 = 0.4;

/// The scramble's re-randomization cadence — the `now - lastUpdate >= 40` gate
/// in `text-scramble.tsx`'s frame loop.
const SCRAMBLE_TICK: Duration = Duration::from_millis(40);

/// Lower bound of the scramble's own duration — `Math.max(420, …)`.
const SCRAMBLE_MIN: Duration = Duration::from_millis(420);
/// Upper bound of the scramble's own duration — `Math.min(760, …)`.
const SCRAMBLE_MAX: Duration = Duration::from_millis(760);
/// Per-character contribution to the scramble's duration — `length * 32`.
const SCRAMBLE_PER_CELL: Duration = Duration::from_millis(32);

/// The glyphs an unresolved scramble position samples from —
/// `DEFAULT_GLYPHS` (`text-scramble.tsx`), transcribed verbatim.
pub const SCRAMBLE_GLYPHS: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ0123456789#%&@$?/";

/// One full shimmer sweep — `duration = 2.5` seconds (`text-shimmer.tsx`).
const SHIMMER_PERIOD: Duration = Duration::from_millis(2500);

/// Where the shimmer's bright stop sits inside its band —
/// `linear-gradient(110deg, muted 30%, foreground 50%, muted 70%)`.
const SHIMMER_HIGHLIGHT_OFFSET: f32 = 0.5;
/// Where the shimmer band's leading muted stop sits.
const SHIMMER_LEAD_OFFSET: f32 = 0.3;
/// Where the shimmer band's trailing muted stop sits.
const SHIMMER_TRAIL_OFFSET: f32 = 0.7;

/// One chromatic sweep — `duration = 1.2` seconds
/// (`chromatic-text-reveal.tsx`).
const CHROMATIC_PERIOD: Duration = Duration::from_millis(1200);

/// Rest between chromatic sweeps when the effect loops — `pauseDuration = 0.8`.
const CHROMATIC_PAUSE: Duration = Duration::from_millis(800);

/// Half-width of the chromatic edge, as a fraction of the run —
/// `TRAIL_HALF_WIDTH = 14` percent.
const CHROMATIC_TRAIL: f64 = 0.14;

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.foreground;
/// Unthemed fallback dimmed ink (beUI light `--muted-foreground`).
const FALLBACK_MUTED_INK: Color = BEUI_LIGHT.muted_foreground;

/// Which of the five upstream text effects a [`TextAnimationView`] plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAnimationVariant {
    /// `text-reveal`: letters rise [`REVEAL_TRAVEL`] of their own height into
    /// place while fading in, [`REVEAL_STAGGER`] apart.
    #[default]
    Reveal,
    /// `text-cascade`: the ported action-swap cascade — letters roll up out of
    /// their slots, [`CASCADE_STAGGER`](crate::motion::chars::CASCADE_STAGGER)
    /// apart, clipped to the line.
    Cascade,
    /// `text-scramble`: the whole string re-randomizes every
    /// [`SCRAMBLE_TICK`], settling left to right.
    Scramble,
    /// `text-shimmer`: a bright band sweeps across the run forever.
    Shimmer,
    /// `chromatic-text-reveal`: a colored edge sweeps left to right, leaving
    /// resolved ink behind it and nothing ahead of it.
    Chromatic,
}

impl TextAnimationVariant {
    /// Every variant, in the order upstream's registry entry lists its examples
    /// — the seam a catalog page or a test enumerates the set through.
    pub const ALL: [TextAnimationVariant; 5] = [
        TextAnimationVariant::Scramble,
        TextAnimationVariant::Chromatic,
        TextAnimationVariant::Reveal,
        TextAnimationVariant::Shimmer,
        TextAnimationVariant::Cascade,
    ];

    /// Whether this variant renders through per-grapheme cells (as opposed to
    /// one shaped run).
    pub fn is_per_letter(self) -> bool {
        matches!(
            self,
            TextAnimationVariant::Reveal | TextAnimationVariant::Cascade
        )
    }
}

/// A declarative beUI text effect. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::components::text_animation::{TextAnimationVariant, text_animation};
///
/// let heading = text_animation::<()>("Ship it")
///     .variant(TextAnimationVariant::Cascade)
///     .size(32.0);
/// ```
pub struct TextAnimationView<State: 'static> {
    content: String,
    variant: TextAnimationVariant,
    size: f32,
    color: Option<Color>,
    duration: Option<Duration>,
    repeat: bool,
    glyphs: String,
    /// The per-letter child, rebuilt by [`TextAnimationView::restage`] whenever
    /// an input it depends on changes. Always present (empty for the whole-run
    /// variants) so a variant swap never has to create or drop a pod.
    cells: AnyView<State>,
}

/// Create a text effect over `content`, playing
/// [`TextAnimationVariant::Reveal`].
pub fn text_animation<State: 'static>(content: impl Into<String>) -> TextAnimationView<State> {
    let mut view = TextAnimationView {
        content: content.into(),
        variant: TextAnimationVariant::default(),
        size: TEXT_BASE as f32,
        color: None,
        duration: None,
        repeat: true,
        glyphs: SCRAMBLE_GLYPHS.to_string(),
        cells: any(char_cascade::<State>("")),
    };
    view.restage();
    view
}

impl<State: 'static> TextAnimationView<State> {
    /// Play `variant` instead of [`TextAnimationVariant::Reveal`].
    pub fn variant(mut self, variant: TextAnimationVariant) -> Self {
        self.variant = variant;
        self.restage();
        self
    }

    /// Set the type size, in logical px (default [`TEXT_BASE`]).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0) as f32;
        self.restage();
        self
    }

    /// Paint in `color` instead of the theme's resolved ink.
    ///
    /// [`Shimmer`](TextAnimationVariant::Shimmer) and
    /// [`Chromatic`](TextAnimationVariant::Chromatic) treat this as their
    /// *settled* color: the swept band is still built from the token hues.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self.restage();
        self
    }

    /// Override the effect's own duration — the scramble's resolve window, the
    /// shimmer's period, the chromatic sweep. Ignored by the two per-letter
    /// variants, whose timing is the stagger's.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Whether a finite effect restarts after it settles (default `true`, which
    /// is upstream's `loop` default on the chromatic reveal). The shimmer is
    /// perpetual regardless; the two per-letter variants play once per string.
    pub fn repeat(mut self, repeat: bool) -> Self {
        self.repeat = repeat;
        self
    }

    /// Sample unresolved scramble positions from `glyphs` instead of
    /// [`SCRAMBLE_GLYPHS`]. An empty set disables the scramble, exactly as
    /// upstream's `!glyphs` guard does.
    pub fn glyphs(mut self, glyphs: impl Into<String>) -> Self {
        self.glyphs = glyphs.into();
        self
    }

    /// The string this view animates.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Rebuild the per-letter child for the current variant and styling.
    ///
    /// Past [`TEXT_ANIMATION_CELL_BUDGET`] graphemes a per-letter variant
    /// stages no cells at all — [`TextAnimationWidget`] falls back to
    /// painting a plain run instead (see the [module docs](self)).
    fn restage(&mut self) {
        let in_budget = self.variant.is_per_letter()
            && !over_budget(self.variant, grapheme_count(&self.content));
        let content: &str = if in_budget { &self.content } else { "" };
        let mut cells: CharCellsView<State> = char_cascade(content);
        cells = cells.size(self.size);
        if let Some(color) = self.color {
            cells = cells.color(color);
        }
        cells = match self.variant {
            TextAnimationVariant::Reveal => cells
                .stagger(Stagger::new(REVEAL_STAGGER, Ramp::spring(SPRING_LAYOUT)))
                .effect(|progress| CellEffect::reveal(progress, REVEAL_TRAVEL)),
            // The cascade is `char_cascade`'s own default staging; clipping to
            // the line is what makes it read as a slot roll rather than a rise.
            _ => cells.clip_to_line(true),
        };
        self.cells = any(cells);
    }
}

/// The retained widget for a [`TextAnimationView`].
pub struct TextAnimationWidget {
    /// The whole string, for the accessibility node and as the scramble's
    /// resolve target.
    content: String,
    cells: ChildPod,
    variant: TextAnimationVariant,
    size: f32,
    color: Option<Color>,
    duration: Option<Duration>,
    repeat: bool,
    glyphs: Vec<char>,
    /// The target string split into cells — how many positions a scramble
    /// resolves, and which of them are whitespace (never scrambled).
    split: CharCells,
    /// The shaped run the whole-run variants paint, plus what it was shaped
    /// from. `None` until the first `layout` with a text context.
    run: Option<TextLayout>,
    shaped: Option<(String, TextStyle)>,
    /// The string the scramble wants shaped next — written by `paint`, consumed
    /// by `layout`.
    display: String,
    /// The frame the current run started at, latched on its first paint.
    started: Option<FrameTime>,
}

impl TextAnimationWidget {
    /// The string this widget animates.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Which effect it plays.
    pub fn variant(&self) -> TextAnimationVariant {
        self.variant
    }

    /// How many cells the string splits into — the count the [module
    /// docs](self)' budget is stated in.
    pub fn cell_count(&self) -> usize {
        self.split.len()
    }

    /// Whether this widget currently renders through per-grapheme cells: a
    /// per-letter variant does, but only within
    /// [`TEXT_ANIMATION_CELL_BUDGET`] graphemes — past that this widget
    /// falls back to painting a plain, non-animating run instead of one
    /// animatable leaf per character (see the [module docs](self)).
    pub fn renders_per_letter(&self) -> bool {
        self.variant.is_per_letter() && !over_budget(self.variant, self.split.len())
    }

    /// The string currently on screen: the scramble's live sample, or the
    /// content itself for every other variant.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The scramble's resolve window: the caller's override, else upstream's
    /// `min(760, max(420, length * 32))`.
    fn scramble_duration(&self) -> Duration {
        self.duration.unwrap_or_else(|| {
            (SCRAMBLE_PER_CELL * self.split.len().min(u32::MAX as usize) as u32)
                .clamp(SCRAMBLE_MIN, SCRAMBLE_MAX)
        })
    }

    /// The style the whole-run variants shape with, in the theme's
    /// `body_medium` family — [`CHAR_CELLS_ROLE`], the role the per-letter
    /// cells resolve, so both routes paint one face. `layout` keys the shaped
    /// run on it, so a theme swap that changes the family reshapes the run.
    fn run_style(&self, theme: Option<&Theme>, ink: Color) -> TextStyle {
        themed_style(TextStyle::new(self.size, ink), CHAR_CELLS_ROLE, theme)
    }

    /// The settled ink: the explicit override, else the theme's `on_surface`,
    /// else [`FALLBACK_INK`].
    fn ink(&self, theme: Option<&Theme>) -> Color {
        self.color
            .unwrap_or_else(|| theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface))
    }

    /// The dimmed ink the shimmer's band fades out to.
    fn muted_ink(&self, theme: Option<&Theme>) -> Color {
        theme.map_or(FALLBACK_MUTED_INK, |t| t.scheme().on_surface_variant)
    }

    /// The five hues the chromatic edge is built from — beUI's own brand set,
    /// standing in for upstream's five Tailwind literals (see the [module
    /// docs](self)).
    fn chromatic_palette(&self, theme: Option<&Theme>) -> [Color; 5] {
        let brightness = theme.map_or(Brightness::Light, |t| t.brightness);
        let tokens = BeuiTokens::resolve(theme);
        let accent = theme.map_or(BEUI_LIGHT.accent, |t| t.scheme().primary_container);
        let gradient = tokens.gradient_accent(brightness);
        [
            accent,
            gradient.to,
            tokens.violet,
            tokens.neon,
            tokens.warning,
        ]
    }

    /// Reset the run so the next paint restarts the effect from its beginning.
    fn restart(&mut self) {
        self.started = None;
        self.display = self.content.clone();
    }
}

impl<State: 'static> View<State> for TextAnimationView<State> {
    type Element = TextAnimationWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TextAnimationWidget {
        let split = CharCells::split(&self.content);
        note_over_budget(self.variant, split.len());
        TextAnimationWidget {
            content: self.content.clone(),
            cells: build_child(&self.cells, ctx),
            variant: self.variant,
            size: self.size,
            color: self.color,
            duration: self.duration,
            repeat: self.repeat,
            glyphs: self.glyphs.chars().collect(),
            split,
            run: None,
            shaped: None,
            display: self.content.clone(),
            started: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TextAnimationWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.cells, &self.cells, &mut element.cells, ctx);

        // The budget notice is keyed on the retained inputs, so only a
        // rebuild that changes them can raise it again.
        let inputs_changed = element.content != self.content || element.variant != self.variant;
        if element.content != self.content {
            element.content = self.content.clone();
            element.split = CharCells::split(&self.content);
            element.restart();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.variant != self.variant {
            element.variant = self.variant;
            element.restart();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if inputs_changed {
            note_over_budget(element.variant, element.split.len());
        }
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        if element.duration != self.duration {
            element.duration = self.duration;
            flags |= ChangeFlags::PAINT;
        }
        if element.repeat != self.repeat {
            element.repeat = self.repeat;
            flags |= ChangeFlags::PAINT;
        }
        let glyphs: Vec<char> = self.glyphs.chars().collect();
        if element.glyphs != glyphs {
            element.glyphs = glyphs;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut TextAnimationWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.cells, &mut element.cells, ctx);
    }
}

impl Widget for TextAnimationWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if self.renders_per_letter() {
            let size = self.cells.layout_child(ctx, bc);
            self.cells.set_origin(Point::ORIGIN);
            return size;
        }

        let ink = self.ink(Theme::from_layout_ctx(ctx));
        let style = self.run_style(Theme::from_layout_ctx(ctx), ink);
        // The scramble shapes whatever `paint` last sampled; every other
        // whole-run variant — and an over-budget per-letter variant falling
        // back to a plain run — shapes the content itself.
        let wanted = if self.variant == TextAnimationVariant::Scramble {
            self.display.clone()
        } else {
            self.content.clone()
        };
        if self.shaped.as_ref() != Some(&(wanted.clone(), style.clone())) {
            let text_ctx = ctx.text_context::<TextContext>();
            self.run = Some(text_ctx.layout(&wanted, &style, None));
            self.shaped = Some((wanted, style));
        }
        let measured = self.run.as_ref().map_or(Size::ZERO, |run| run.size());
        bc.constrain(measured)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);

        if self.renders_per_letter() {
            // The cells own their own timing, their own frame requests and their
            // own reduced-motion collapse.
            self.cells.paint_child(ctx, scene);
            return;
        }

        let now = ctx.frame_time();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let ink = self.ink(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        match self.variant {
            // In-budget cells are handled by the early return above; reaching
            // this arm means the cell budget was exceeded (see
            // `renders_per_letter`) — a plain, non-animating run instead of
            // one leaf per character.
            TextAnimationVariant::Reveal | TextAnimationVariant::Cascade => {
                paint_run(self.run.as_ref(), origin, &Brush::Solid(ink), scene);
            }
            TextAnimationVariant::Scramble => {
                let duration = self.scramble_duration();
                let wanted = if reduce || self.glyphs.is_empty() {
                    self.content.clone()
                } else {
                    scramble_display(&self.split, &self.glyphs, elapsed, duration)
                };
                if wanted != self.display {
                    self.display = wanted;
                    // A different string is a different shaped run, and shaping
                    // only happens in `layout`.
                    ctx.request_layout();
                }
                paint_run(self.run.as_ref(), origin, &Brush::Solid(ink), scene);
                if !reduce && !self.glyphs.is_empty() && elapsed < duration {
                    // Far slower than the frame gate's cosmetic cap, so the
                    // cadence is stated rather than left to the gate.
                    ctx.request_frame_paced_at(SCRAMBLE_TICK);
                }
            }
            TextAnimationVariant::Shimmer => {
                if reduce {
                    // Upstream's own reduced-motion rule is `animation: none`,
                    // which leaves the gradient's first stop showing.
                    paint_run(self.run.as_ref(), origin, &Brush::Solid(ink), scene);
                    return;
                }
                let period = self.duration.unwrap_or(SHIMMER_PERIOD);
                let phase = cycle(elapsed, period);
                let gradient = shimmer_gradient(origin, size, phase, self.muted_ink(theme), ink);
                paint_run(self.run.as_ref(), origin, &Brush::Gradient(gradient), scene);
                // A perpetual decorative loop, never a transition.
                ctx.request_frame_class(TickClass::CosmeticLoop);
            }
            TextAnimationVariant::Chromatic => {
                let period = self.duration.unwrap_or(CHROMATIC_PERIOD);
                if reduce {
                    paint_run(self.run.as_ref(), origin, &Brush::Solid(ink), scene);
                    return;
                }
                let cycle_length = if self.repeat {
                    period + CHROMATIC_PAUSE
                } else {
                    period
                };
                let into = if self.repeat {
                    Duration::from_secs_f64(
                        cycle(elapsed, cycle_length) * cycle_length.as_secs_f64(),
                    )
                } else {
                    elapsed
                };
                let progress = if period.is_zero() {
                    1.0
                } else {
                    EASE_IN_OUT
                        .transform((into.as_secs_f64() / period.as_secs_f64()).clamp(0.0, 1.0))
                };
                let sweep = chromatic_sweep(progress);
                let gradient =
                    chromatic_gradient(origin, size, sweep, ink, &self.chromatic_palette(theme));
                paint_run(self.run.as_ref(), origin, &Brush::Gradient(gradient), scene);
                if self.repeat || into < period {
                    ctx.request_frame();
                }
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Non-interactive; a broadcast still has to reach the cells so their
        // pods stay live.
        if event.is_broadcast() {
            self.cells.event_child(ctx, event);
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The resolved string, never the scramble's live sample — upstream
        // publishes the same thing through an `sr-only` span while the visible
        // text is `aria-hidden`.
        ctx.push_container(
            Role::Label,
            |node| node.set_value(self.content.as_str()),
            |ctx| {
                if self.renders_per_letter() {
                    self.cells.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(cells);
}

/// `elapsed` folded into `[0, 1)` of a `period`-long loop. A zero period has no
/// cycle and reports `0.0`.
fn cycle(elapsed: Duration, period: Duration) -> f64 {
    if period.is_zero() {
        return 0.0;
    }
    let turns = elapsed.as_secs_f64() / period.as_secs_f64();
    turns - turns.floor()
}

/// The scramble's sampled string at `elapsed`.
///
/// Positions resolve left to right: the leading `floor(progress * count)` cells
/// carry their real text, and so does every whitespace cell at every instant
/// (upstream's `character === " "` guard, widened here to all whitespace).
/// Everything else samples `glyphs`.
///
/// Deterministic: the sampled index is a hash of the cell's position and the
/// tick number, not a random draw, so the same clock always produces the same
/// string — which is what makes the effect testable at all.
fn scramble_display(
    cells: &CharCells,
    glyphs: &[char],
    elapsed: Duration,
    duration: Duration,
) -> String {
    let count = cells.len();
    if count == 0 || glyphs.is_empty() {
        return cells.source().to_string();
    }
    let progress = if duration.is_zero() {
        1.0
    } else {
        (elapsed.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0)
    };
    let settled = (progress * count as f64).floor() as usize;
    let tick = (elapsed.as_millis() / SCRAMBLE_TICK.as_millis().max(1)) as u64;

    let mut out = String::with_capacity(cells.source().len());
    for (index, cell) in cells.cells().iter().enumerate() {
        let literal = index < settled || cell.text().chars().all(char::is_whitespace);
        if literal {
            out.push_str(cell.text());
        } else {
            let pick = (mix(tick, index as u64) % glyphs.len() as u64) as usize;
            out.push(glyphs[pick]);
        }
    }
    out
}

/// A cheap, well-mixed hash of two counters — the scramble's determinism seam.
/// SplitMix64's finalizer over `tick` salted by `index`; the constants are that
/// algorithm's own.
fn mix(tick: u64, index: u64) -> u64 {
    let mut z = tick
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(index.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The shimmer's band at `phase`: a `base`-to-`highlight`-to-`base` gradient two
/// run-widths wide, travelling left to right across the run.
///
/// Built per frame because a gradient's geometry is resolved where it is
/// encoded: the stops stay put and the band's endpoints move.
fn shimmer_gradient(
    origin: Point,
    size: Size,
    phase: f64,
    base: Color,
    highlight: Color,
) -> Gradient {
    let band = (size.width * 2.0).max(1.0);
    // At phase 0 the band sits entirely off the right edge; at phase 1 entirely
    // off the left, so the bright stop crosses the whole run exactly once.
    let start = origin.x + size.width - phase * (size.width + band);
    let mid_y = origin.y + size.height / 2.0;
    Gradient::new_linear(Point::new(start, mid_y), Point::new(start + band, mid_y)).with_stops(
        [
            ColorStop::from((0.0f32, base)),
            ColorStop::from((SHIMMER_LEAD_OFFSET, base)),
            ColorStop::from((SHIMMER_HIGHLIGHT_OFFSET, highlight)),
            ColorStop::from((SHIMMER_TRAIL_OFFSET, base)),
            ColorStop::from((1.0f32, base)),
        ]
        .as_slice(),
    )
}

/// Where the chromatic edge sits at `progress`, as a fraction of the run:
/// `-TRAIL_HALF_WIDTH` to `1 + TRAIL_HALF_WIDTH`, so the edge starts fully off
/// the leading side and finishes fully off the trailing one — upstream's
/// `REVEAL_START`/`REVEAL_FINISH`.
fn chromatic_sweep(progress: f64) -> f64 {
    let from = -CHROMATIC_TRAIL;
    let to = 1.0 + CHROMATIC_TRAIL;
    from + (to - from) * progress
}

/// The chromatic edge at `sweep`: resolved `ink` behind it, `palette` across it,
/// nothing ahead of it.
///
/// Offsets are clamped into `[0, 1]` and forced non-decreasing, because the edge
/// legitimately hangs off both ends of the run and a gradient's stop list may
/// not go backwards.
fn chromatic_gradient(
    origin: Point,
    size: Size,
    sweep: f64,
    ink: Color,
    palette: &[Color],
) -> Gradient {
    let mid_y = origin.y + size.height / 2.0;
    let mut stops: Vec<ColorStop> = Vec::with_capacity(palette.len() + 4);
    let mut last = 0.0f32;
    let mut push = |offset: f64, color: Color, last: &mut f32| {
        let clamped = (offset.clamp(0.0, 1.0) as f32).max(*last);
        *last = clamped;
        stops.push(ColorStop::from((clamped, color)));
    };
    push(0.0, ink, &mut last);
    push(sweep - CHROMATIC_TRAIL, ink, &mut last);
    for (index, color) in palette.iter().enumerate() {
        let spread = if palette.len() == 1 {
            0.0
        } else {
            index as f64 / (palette.len() - 1) as f64
        };
        push(
            sweep - CHROMATIC_TRAIL + spread * CHROMATIC_TRAIL * 2.0,
            *color,
            &mut last,
        );
    }
    push(sweep + CHROMATIC_TRAIL, with_alpha(ink, 0.0), &mut last);
    push(1.0, with_alpha(ink, 0.0), &mut last);
    Gradient::new_linear(
        Point::new(origin.x, mid_y),
        Point::new(origin.x + size.width.max(1.0), mid_y),
    )
    .with_stops(stops.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust_core::{BuildCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays its run into.
    const BOX: Size = Size::new(240.0, 24.0);

    /// Records the brush every glyph run was drawn with.
    #[derive(Default)]
    struct Recorder {
        brushes: Vec<Brush>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.brushes.push(run.brush);
        }
    }

    /// Build `view` into its widget and lay it out into [`BOX`] with a real
    /// text context, so the whole-run variants have something shaped to paint.
    fn laid_out(view: &TextAnimationView<()>) -> TextAnimationWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut TextAnimationWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
    }

    /// Paint `widget` at `ms` on a synthetic clock.
    fn painted(
        widget: &mut TextAnimationWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, Option<TickClass>, Option<Duration>, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (
            recorder,
            ctx.needs_frame(),
            ctx.frame_class(),
            ctx.paced_interval(),
            ctx.needs_layout(),
        )
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Every example upstream's registry entry lists is constructible, and the
    /// enumeration seam covers the whole set exactly once.
    #[test]
    fn every_registry_variant_is_constructible() {
        assert_eq!(TextAnimationVariant::ALL.len(), 5);
        for variant in TextAnimationVariant::ALL {
            let view = text_animation::<()>("Ship it").variant(variant);
            let widget = laid_out(&view);
            assert_eq!(widget.variant(), variant);
            assert_eq!(widget.content(), "Ship it");
        }
        // No arm repeats.
        for (index, variant) in TextAnimationVariant::ALL.iter().enumerate() {
            assert!(
                !TextAnimationVariant::ALL[..index].contains(variant),
                "{variant:?} listed twice"
            );
        }
    }

    /// The two-route split the module docs describe, pinned: only the reveal
    /// and the cascade animate letters individually.
    #[test]
    fn only_the_reveal_and_cascade_render_per_letter() {
        for variant in TextAnimationVariant::ALL {
            let per_letter = matches!(
                variant,
                TextAnimationVariant::Reveal | TextAnimationVariant::Cascade
            );
            assert_eq!(variant.is_per_letter(), per_letter, "{variant:?}");
        }
    }

    /// Past [`TEXT_ANIMATION_CELL_BUDGET`] graphemes a per-letter variant
    /// falls back to painting a plain, non-animating run rather than staging
    /// one cell per character.
    #[test]
    fn past_the_cell_budget_a_per_letter_variant_falls_back_to_a_plain_run() {
        let long = "x".repeat(TEXT_ANIMATION_CELL_BUDGET + 1);
        for variant in [TextAnimationVariant::Reveal, TextAnimationVariant::Cascade] {
            let view = text_animation::<()>(long.clone()).variant(variant);
            let mut widget = laid_out(&view);
            assert_eq!(widget.cell_count(), TEXT_ANIMATION_CELL_BUDGET + 1);
            assert!(
                !widget.renders_per_letter(),
                "{variant:?} over budget must not render per letter"
            );

            let (rec, needs_frame, ..) = painted(&mut widget, 0, None);
            assert!(
                !rec.brushes.is_empty(),
                "{variant:?} over budget still paints a shaped run"
            );
            assert!(
                !needs_frame,
                "{variant:?}'s plain-run fallback asks for no further frames"
            );
        }

        // Exactly at the budget still renders per letter; the ceiling is
        // exclusive.
        let at_budget = "x".repeat(TEXT_ANIMATION_CELL_BUDGET);
        let widget =
            laid_out(&text_animation::<()>(at_budget).variant(TextAnimationVariant::Reveal));
        assert_eq!(widget.cell_count(), TEXT_ANIMATION_CELL_BUDGET);
        assert!(widget.renders_per_letter());

        // A short string plainly stays on the per-letter route.
        let short =
            laid_out(&text_animation::<()>("Ship it").variant(TextAnimationVariant::Cascade));
        assert!(short.renders_per_letter());
    }

    /// The budget predicate the fallback and its debug notice share: only a
    /// per-letter variant can be over budget, and the ceiling is exclusive.
    #[test]
    fn only_a_per_letter_variant_past_the_budget_is_over_budget() {
        assert!(over_budget(
            TextAnimationVariant::Reveal,
            TEXT_ANIMATION_CELL_BUDGET + 1
        ));
        assert!(over_budget(
            TextAnimationVariant::Cascade,
            TEXT_ANIMATION_CELL_BUDGET + 1
        ));
        assert!(!over_budget(
            TextAnimationVariant::Reveal,
            TEXT_ANIMATION_CELL_BUDGET
        ));
        assert!(!over_budget(TextAnimationVariant::Scramble, usize::MAX));
        assert!(!over_budget(TextAnimationVariant::Shimmer, usize::MAX));
    }

    /// A rebuild with unchanged inputs changes nothing on the retained widget
    /// — the over-budget notice is keyed on those inputs, so it cannot fire
    /// again either.
    #[test]
    fn a_redundant_rebuild_of_an_over_budget_widget_changes_nothing() {
        let long = "x".repeat(TEXT_ANIMATION_CELL_BUDGET + 1);
        let view = text_animation::<()>(long.clone()).variant(TextAnimationVariant::Reveal);
        let mut widget = laid_out(&view);
        let mut next_id = 0u64;
        let flags =
            View::<()>::rebuild(&view, &view, &mut widget, &mut BuildCtx::new(&mut next_id));
        assert_eq!(flags, ChangeFlags::NONE);
        assert_eq!(widget.content(), long);
        assert!(!widget.renders_per_letter());
    }

    /// A per-letter variant splits the string into one cell per grapheme —
    /// combining marks and emoji included, since the split is the substrate's.
    #[test]
    fn the_cell_count_is_one_per_grapheme() {
        let plain = laid_out(&text_animation::<()>("Ship it"));
        assert_eq!(plain.cell_count(), 7);

        // "cafe" + combining acute, then a flag: four letters and one flag.
        let clustered = laid_out(&text_animation::<()>("cafe\u{0301}\u{1F1EC}\u{1F1E7}"));
        assert_eq!(clustered.cell_count(), 5);

        // The count is the *string's*, not the variant's: a whole-run variant
        // reports the same split (the scramble resolves position by position).
        let run =
            laid_out(&text_animation::<()>("Ship it").variant(TextAnimationVariant::Scramble));
        assert_eq!(run.cell_count(), 7);
    }

    /// The scramble resolves left to right and never disturbs whitespace, so
    /// the word boundaries of the final string are legible the whole way.
    #[test]
    fn the_scramble_settles_left_to_right_and_leaves_spaces_alone() {
        let cells = CharCells::split("SHIP IT NOW");
        let glyphs: Vec<char> = SCRAMBLE_GLYPHS.chars().collect();
        let duration = Duration::from_millis(440);

        let start = scramble_display(&cells, &glyphs, Duration::ZERO, duration);
        assert_eq!(start.chars().count(), cells.len());
        assert_eq!(
            start.chars().nth(4),
            Some(' '),
            "whitespace is never scrambled"
        );
        assert_eq!(start.chars().nth(7), Some(' '));

        // Half way, the leading half is resolved and the tail is not yet.
        let middle = scramble_display(&cells, &glyphs, duration.mul_f64(0.5), duration);
        assert!(
            middle.starts_with("SHIP "),
            "leading half resolved: {middle}"
        );

        // At and past its duration it is exactly the target.
        assert_eq!(
            scramble_display(&cells, &glyphs, duration, duration),
            "SHIP IT NOW"
        );
        assert_eq!(
            scramble_display(&cells, &glyphs, duration * 4, duration),
            "SHIP IT NOW"
        );
    }

    /// The sample is a pure function of the clock — the property that makes the
    /// effect testable and a frame replay reproducible.
    #[test]
    fn the_scramble_is_deterministic_and_actually_scrambles() {
        let cells = CharCells::split("SHIPIT");
        let glyphs: Vec<char> = SCRAMBLE_GLYPHS.chars().collect();
        let duration = Duration::from_millis(400);
        let at = Duration::from_millis(80);

        let once = scramble_display(&cells, &glyphs, at, duration);
        assert_eq!(once, scramble_display(&cells, &glyphs, at, duration));
        assert_ne!(once, "SHIPIT", "an early sample is not the target");

        // Every unresolved position holds a glyph from the sampled set.
        for character in once.chars().skip(2) {
            assert!(
                glyphs.contains(&character) || "SHIPIT".contains(character),
                "{character} is not from the glyph set"
            );
        }

        // An empty glyph set disables the effect outright (upstream's `!glyphs`
        // guard).
        assert_eq!(scramble_display(&cells, &[], at, duration), "SHIPIT");
    }

    /// A scramble asks for its own slow cadence while it resolves, re-shapes
    /// when the sample changes, and goes quiet once it has settled.
    #[test]
    fn the_scramble_paces_its_own_ticks_and_stops_when_resolved() {
        let mut widget =
            laid_out(&text_animation::<()>("SHIP IT").variant(TextAnimationVariant::Scramble));
        let (_, needs_frame, _, paced, needs_layout) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        assert_eq!(paced, Some(SCRAMBLE_TICK));
        assert!(needs_layout, "a changed sample needs re-shaping");
        assert_ne!(widget.display(), "SHIP IT");

        // Past its own resolve window the sample becomes the target. That last
        // change still needs one re-shape...
        let (_, _, _, _, needs_layout) = painted(&mut widget, 5_000, None);
        assert!(needs_layout);
        assert_eq!(widget.display(), "SHIP IT");
        // ...and once it is shaped, the effect asks for nothing at all.
        relayout(&mut widget);
        let (_, needs_frame, _, _, needs_layout) = painted(&mut widget, 5_100, None);
        assert!(!needs_frame);
        assert!(!needs_layout);
    }

    /// `reduce_motion` collapses the scramble to its resolved text with no
    /// frames at all — upstream's own `reduce` branch.
    #[test]
    fn reduce_motion_resolves_the_scramble_immediately() {
        let theme = reduced();
        let mut widget =
            laid_out(&text_animation::<()>("SHIP IT").variant(TextAnimationVariant::Scramble));
        let (_, needs_frame, _, _, _) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(widget.display(), "SHIP IT");
    }

    /// The shimmer is a perpetual decorative loop, so it asks for a paceable
    /// cosmetic frame rather than a transition one, and paints its run with a
    /// gradient rather than a solid.
    #[test]
    fn the_shimmer_loops_cosmetically_under_a_gradient() {
        let mut widget =
            laid_out(&text_animation::<()>("Loading").variant(TextAnimationVariant::Shimmer));
        let (recorder, needs_frame, class, _, _) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        assert_eq!(class, Some(TickClass::CosmeticLoop));
        assert!(!recorder.brushes.is_empty(), "the run painted");
        for brush in &recorder.brushes {
            assert!(matches!(brush, Brush::Gradient(_)));
        }
    }

    /// ...and collapses to a plain solid fill with no frames under
    /// `reduce_motion` (upstream's `animation: none`).
    #[test]
    fn reduce_motion_freezes_the_shimmer_solid() {
        let theme = reduced();
        let mut widget =
            laid_out(&text_animation::<()>("Loading").variant(TextAnimationVariant::Shimmer));
        let (recorder, needs_frame, _, _, _) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        for brush in &recorder.brushes {
            assert!(matches!(brush, Brush::Solid(_)));
        }
    }

    /// The shimmer's band is re-encoded per frame — the gradient's *geometry*
    /// moves, which is the whole reason it cannot be built once.
    #[test]
    fn the_shimmer_band_travels_with_its_phase() {
        let origin = Point::new(10.0, 5.0);
        let start = shimmer_gradient(origin, BOX, 0.0, Color::BLACK, Color::WHITE);
        let later = shimmer_gradient(origin, BOX, 0.5, Color::BLACK, Color::WHITE);
        assert_ne!(
            format!("{:?}", start.kind),
            format!("{:?}", later.kind),
            "the band did not move"
        );
        assert_eq!(start.stops.len(), 5);
    }

    /// The chromatic edge starts fully clear of the leading side and finishes
    /// fully clear of the trailing one, so no colour is stranded on screen.
    #[test]
    fn the_chromatic_edge_sweeps_clear_of_both_ends() {
        assert_eq!(chromatic_sweep(0.0), -CHROMATIC_TRAIL);
        assert_eq!(chromatic_sweep(1.0), 1.0 + CHROMATIC_TRAIL);
        assert!(chromatic_sweep(0.5) > 0.0 && chromatic_sweep(0.5) < 1.0);
    }

    /// However far off either end the edge sits, the stop list stays a legal
    /// gradient: inside `[0, 1]` and never going backwards.
    #[test]
    fn the_chromatic_stops_stay_ordered_at_every_sweep_position() {
        let palette = [Color::WHITE; 5];
        for step in -20..=120 {
            let sweep = f64::from(step) / 100.0;
            let gradient = chromatic_gradient(Point::ORIGIN, BOX, sweep, Color::BLACK, &palette);
            let mut previous = 0.0f32;
            for stop in gradient.stops.iter() {
                assert!(
                    (0.0..=1.0).contains(&stop.offset),
                    "offset {} out of range at sweep {sweep}",
                    stop.offset
                );
                assert!(
                    stop.offset >= previous,
                    "stops went backwards at sweep {sweep}"
                );
                previous = stop.offset;
            }
        }
        // A single-hue palette is legal too (the spread division would be a
        // divide-by-zero if it were taken unguarded).
        let single = chromatic_gradient(Point::ORIGIN, BOX, 0.5, Color::BLACK, &[Color::WHITE]);
        assert!(!single.stops.is_empty());
    }

    /// A looping chromatic reveal keeps asking for frames; a one-shot stops
    /// asking once its sweep is done.
    #[test]
    fn the_chromatic_reveal_loops_only_when_asked() {
        let mut looping =
            laid_out(&text_animation::<()>("beUI").variant(TextAnimationVariant::Chromatic));
        let (_, needs_frame, _, _, _) = painted(&mut looping, 10_000, None);
        assert!(needs_frame, "a looping reveal never settles");

        let mut once = laid_out(
            &text_animation::<()>("beUI")
                .variant(TextAnimationVariant::Chromatic)
                .repeat(false),
        );
        let (_, early, _, _, _) = painted(&mut once, 0, None);
        assert!(early);
        let (_, late, _, _, _) = painted(&mut once, 10_000, None);
        assert!(!late, "a one-shot reveal stops when the sweep clears");
    }

    /// A per-letter variant delegates its whole timing to the cells, which own
    /// the frame requests — the component itself adds none.
    #[test]
    fn a_per_letter_variant_delegates_to_the_cells() {
        for variant in [TextAnimationVariant::Reveal, TextAnimationVariant::Cascade] {
            let mut widget = laid_out(&text_animation::<()>("Ship it").variant(variant));
            let (_, needs_frame, _, _, _) = painted(&mut widget, 0, None);
            assert!(needs_frame, "{variant:?} cells are still cascading");

            let theme = reduced();
            let (_, reduced_frame, _, _, _) = painted(&mut widget, 0, Some(&theme));
            assert!(!reduced_frame, "{variant:?} collapses under reduce_motion");
        }
    }

    /// A changed string restarts the effect rather than continuing a run timed
    /// from the old one — upstream's own trigger.
    #[test]
    fn a_changed_string_restarts_the_run() {
        let first = text_animation::<()>("one").variant(TextAnimationVariant::Scramble);
        let mut widget = laid_out(&first);
        // The run is timed from its first paint, so start the clock and then
        // run it past the resolve window.
        painted(&mut widget, 0, None);
        painted(&mut widget, 5_000, None);
        assert_eq!(widget.display(), "one");

        let second = text_animation::<()>("two").variant(TextAnimationVariant::Scramble);
        let mut next_id = 0u64;
        View::<()>::rebuild(
            &second,
            &first,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        relayout(&mut widget);
        assert_eq!(widget.content(), "two");
        let (_, needs_frame, _, _, _) = painted(&mut widget, 5_100, None);
        assert!(needs_frame, "the new string scrambles from its own start");
    }

    /// The loop fold is a real modulo: it wraps and never leaves `[0, 1)`.
    #[test]
    fn the_cycle_fold_wraps_without_leaving_the_unit_interval() {
        let period = Duration::from_millis(1000);
        assert_eq!(cycle(Duration::ZERO, period), 0.0);
        assert!((cycle(Duration::from_millis(500), period) - 0.5).abs() < 1e-9);
        assert!(cycle(Duration::from_millis(1000), period).abs() < 1e-9);
        assert!((cycle(Duration::from_millis(2500), period) - 0.5).abs() < 1e-9);
        // A degenerate period has no cycle rather than dividing by zero.
        assert_eq!(cycle(Duration::from_millis(10), Duration::ZERO), 0.0);
    }

    /// The scramble's own resolve window follows upstream's length rule, and a
    /// caller's override wins.
    #[test]
    fn the_scramble_window_follows_the_source_clamp() {
        let short = laid_out(&text_animation::<()>("hi").variant(TextAnimationVariant::Scramble));
        assert_eq!(short.scramble_duration(), SCRAMBLE_MIN, "clamped up");

        let long = laid_out(
            &text_animation::<()>(&"x".repeat(40)).variant(TextAnimationVariant::Scramble),
        );
        assert_eq!(long.scramble_duration(), SCRAMBLE_MAX, "clamped down");

        let overridden = laid_out(
            &text_animation::<()>("hi")
                .variant(TextAnimationVariant::Scramble)
                .duration(Duration::from_millis(100)),
        );
        assert_eq!(overridden.scramble_duration(), Duration::from_millis(100));
    }

    // ---- Typeface: both rendering routes follow the live theme -------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// The window every typeface probe paints into.
    const PROBE_WINDOW: Size = Size::new(240.0, 60.0);

    /// `variant` over a short string, so the per-letter variants render cells
    /// and the whole-run ones one shaped run.
    fn probe_logic(variant: TextAnimationVariant) -> impl FnMut(&mut ()) -> TextAnimationView<()> {
        move |_: &mut ()| text_animation::<()>("Ship").variant(variant)
    }

    /// A per-letter variant past the cell budget, which paints its plain-run
    /// fallback.
    fn over_budget_logic(_: &mut ()) -> TextAnimationView<()> {
        text_animation::<()>("a".repeat(TEXT_ANIMATION_CELL_BUDGET + 1))
            .variant(TextAnimationVariant::Reveal)
    }

    #[test]
    fn every_variant_paints_in_geist_under_the_beui_theme() {
        for variant in TextAnimationVariant::ALL {
            assert_paints_only_in_geist(
                &format!("{variant:?}"),
                probe_logic(variant),
                PROBE_WINDOW,
            );
        }
        assert_paints_only_in_geist("the over-budget fallback", over_budget_logic, PROBE_WINDOW);
    }

    #[test]
    fn every_variant_follows_a_live_theme_family_swap() {
        for variant in TextAnimationVariant::ALL {
            assert_follows_a_live_family_swap(
                &format!("{variant:?}"),
                probe_logic(variant),
                PROBE_WINDOW,
            );
        }
        assert_follows_a_live_family_swap(
            "the over-budget fallback",
            over_budget_logic,
            PROBE_WINDOW,
        );
    }
}
