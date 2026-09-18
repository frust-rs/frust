//! Per-character text cells: a string split into graphemes, laid out on one
//! line, each cell independently animatable off a [`Stagger`] slot.
//!
//! The catalog's letter-by-letter effects — the text cascade, the reveal, the
//! shimmer sweep, the scramble — are all the same shape on the web: the string
//! is emitted as one inline-block `<span>` per letter and a parent variant gives
//! each span its own delay (`components/motion/action-swap.tsx`'s
//! `CASCADE_LETTER_VARIANTS`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`). This module is that split, plus
//! the widget that lays the cells out and composites each one.
//!
//! # The shaping compromise, stated plainly
//!
//! **A cell is shaped on its own.** Text shaping runs per cell, so every
//! cross-cell typographic relationship is lost: kerning pairs (`AV`, `To`) no
//! longer tighten, ligatures (`fi`, `ffl`) no longer form, and a cursive or
//! complex script (Arabic, Devanagari) loses the joining and reordering that
//! makes it legible at all. The same string through an ordinary
//! [`text`](frust::text) run is shaped once and keeps all of it.
//!
//! That is inherent, not an implementation shortcut — the effect *requires* each
//! letter to be independently transformable, and a single shaped run has no
//! independently transformable letters. Upstream carries the identical
//! compromise for the identical reason (its own note: per-letter spans are
//! `inline-block` "so proportional glyph widths never jitter").
//!
//! **So: effect text only.** Short display strings — a heading, a label, a
//! counter, a button's swapping caption. **Never body text**, never a paragraph,
//! never user-supplied content in a script this compromise breaks. A component
//! reaching for this on a block of prose has chosen the wrong primitive.
//!
//! # Two more consequences of the split
//!
//! * **One line, no wrapping.** Cells are laid out left to right and the widget
//!   reports their total width; nothing here breaks lines, because a line break
//!   between two independently-animated cells has no meaning the effect could
//!   preserve. A caller sizing this into a narrow box gets overflow, not a wrap.
//! * **Colour is a build-time property, not an animated one.** A cell is a real
//!   text leaf, and a leaf's colour is set when it is built — so a per-cell
//!   colour *change* travels through a rebuild, while opacity, offset and scale
//!   are applied at paint by [`CellEffect`] and cost nothing. An effect that
//!   wants a colour sweep drives the rebuild; one that wants a brightness sweep
//!   should use alpha instead and stay on the paint path.
//!
//! # The family comes from the theme
//!
//! Each cell leaf opts into `frust::text`'s `.themed_family(role)`, so its
//! family is read from the live theme's type scale at layout. The role is
//! [`CHAR_CELLS_ROLE`] unless [`CharCellsView::themed_family`] names another.
//! The leaf keys its shaped run on the resolved style, so a theme swap
//! reshapes every cell. [`CharCellsView::style`] names the family in code and
//! wins over the theme, as `Text::style` does.
//!
//! # Grapheme splitting is an approximation
//!
//! Splitting on `char` boundaries would break every combining sequence in the
//! language: `e` + U+0301 would animate its accent away from its letter, and a
//! flag or skin-toned emoji would come apart into its components.
//! [`CharCells::split`] therefore clusters, but it does so with a **documented
//! subset** of Unicode's extended grapheme rules rather than the whole
//! algorithm, because this crate ships no segmentation dependency and one is not
//! being added for an effect. See [`CharCells::split`] for exactly what is and
//! is not clustered.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontStyle, TextStyle};
use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, Vec2, View,
    Widget, any, build_child, rebuild_children, teardown_child, visit_children,
};
use frust::{Color, FrameTime, TextView, Theme, text};

use super::stagger::{Stagger, StaggerDirection};
use crate::text::ThemeTextType;

/// The type-scale role a cell's family resolves from by default.
///
/// `body_medium`: the role `text_animation` shapes its whole-run variants in,
/// so that component paints one face whether it renders per letter or not.
pub const CHAR_CELLS_ROLE: ThemeTextType = ThemeTextType::BodyMedium;

/// The vertical travel of a cascading letter, as a fraction of its own height.
///
/// Source: `CASCADE_LETTER_VARIANTS`' `y: "105%"` on both the initial and exit
/// states (`components/motion/action-swap.tsx`) — slightly more than a full cell
/// height, so a letter is fully clear of its slot before it stops moving.
pub const CASCADE_TRAVEL: f64 = 1.05;

/// The gap between consecutive letters in a cascade.
///
/// Source: `CASCADE_STAGGER = 0.025` (`components/motion/action-swap.tsx`).
pub const CASCADE_STAGGER: Duration = Duration::from_millis(25);

/// How far outside its own box a cell's composited layer is allowed to reach,
/// as a multiple of the widget's line height.
///
/// A cascading letter travels [`CASCADE_TRAVEL`] of its height and a scaling one
/// grows past its box, so a layer bounded to the cell rect would cut the effect
/// off mid-motion on a backend that treats the layer rect as a bound. One line
/// height of headroom in every direction covers every effect this module
/// publishes; a caller wanting the letters clipped back to their line asks for
/// it explicitly with [`CharCellsView::clip_to_line`].
const CELL_LAYER_BLEED: f64 = 1.0;

/// One cell of a split string: the text it carries and where that text came
/// from in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharCell {
    text: String,
    start: usize,
}

impl CharCell {
    /// The cell's own text — one grapheme cluster as [`CharCells::split`]
    /// defines them, which may be several `char`s.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The byte offset this cell begins at in the string it was split from.
    pub fn start(&self) -> usize {
        self.start
    }

    /// The byte range this cell occupies in the string it was split from.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.start + self.text.len()
    }
}

/// A string split into per-grapheme cells.
///
/// Keeps the source alongside the cells so a consumer can still publish the
/// whole string — which is exactly what [`CharCellsWidget`] does for
/// accessibility, since a screen reader must never be handed the letters
/// individually.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharCells {
    source: String,
    cells: Vec<CharCell>,
}

impl CharCells {
    /// Split `content` into per-grapheme cells.
    ///
    /// # What clusters
    ///
    /// A cell starts at each `char` and absorbs the run that follows it:
    ///
    /// * **Combining marks** — the Combining Diacritical Marks blocks and their
    ///   extensions/supplements, the Cyrillic, Hebrew, Arabic and Thai mark
    ///   ranges, the combining marks for symbols (which is what makes a keycap
    ///   sequence one cell), and the combining half marks. So `e` + U+0301 is
    ///   one cell, not two.
    /// * **Variation selectors**, both the base block and its supplement, plus
    ///   the tag characters emoji subdivision flags are built from.
    /// * **Zero-width joiner sequences** — a ZWJ absorbs whatever follows it, so
    ///   a multi-person or professional emoji stays one cell.
    /// * **Emoji modifiers** — the five skin-tone modifiers attach to the emoji
    ///   they modify.
    /// * **Regional indicator pairs** — two indicators make one flag; a third
    ///   starts a new cell, so a run of flags splits where it should.
    /// * **CRLF**, kept together as one cell.
    ///
    /// # What does not
    ///
    /// This is a **subset** of Unicode's extended grapheme cluster algorithm,
    /// covering the sequences a Latin-script effect string and the emoji a
    /// caption might carry actually contain. It does **not** implement Indic
    /// consonant clusters (a Devanagari matra or virama sequence splits from its
    /// consonant), Hangul syllable composition, or prepended concatenation
    /// marks. Combined with the shaping compromise the [module docs](self)
    /// describe — which breaks those scripts' rendering regardless — the
    /// conclusion is the same one stated there: this is for short Latin-script
    /// effect text, not arbitrary content.
    pub fn split(content: &str) -> Self {
        let mut cells: Vec<CharCell> = Vec::new();
        let mut chars = content.char_indices().peekable();

        while let Some((start, first)) = chars.next() {
            let mut end = start + first.len_utf8();
            let mut previous = first;
            let mut regional_run = usize::from(is_regional_indicator(first));

            while let Some(&(_, next)) = chars.peek() {
                let joins = if previous == ZERO_WIDTH_JOINER {
                    // A joiner binds whatever comes after it, whatever that is.
                    true
                } else if is_regional_indicator(previous) && is_regional_indicator(next) {
                    // Flags pair up; a third indicator opens the next flag.
                    regional_run % 2 == 1
                } else if previous == '\r' && next == '\n' {
                    true
                } else {
                    is_extending(next)
                };
                if !joins {
                    break;
                }
                if is_regional_indicator(next) {
                    regional_run += 1;
                }
                previous = next;
                end += next.len_utf8();
                chars.next();
            }

            cells.push(CharCell {
                text: content[start..end].to_string(),
                start,
            });
        }

        CharCells {
            source: content.to_string(),
            cells,
        }
    }

    /// The string these cells were split from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The cells, in reading order.
    pub fn cells(&self) -> &[CharCell] {
        &self.cells
    }

    /// How many cells the string split into.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the string split into no cells at all (it was empty).
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// What one cell looks like at a given point in its own sub-animation.
///
/// Pure staging numbers, applied at paint: the offsets are expressed in
/// **fractions of the cell's own size**, mirroring the percentage transforms
/// upstream authors (`y: "105%"`), so an effect is written once and reads the
/// same at any type size.
///
/// Colour is deliberately absent — see the [module docs](self) on why it travels
/// through a rebuild instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellEffect {
    /// Opacity, `0.0..=1.0`. Values outside are clamped when composited.
    pub alpha: f64,
    /// Translation, in fractions of the cell's own width and height.
    pub offset: Vec2,
    /// Uniform scale about the cell's centre.
    pub scale: f64,
}

impl CellEffect {
    /// Fully present and untransformed — the settled state of every enter
    /// effect.
    pub const IDENTITY: CellEffect = CellEffect {
        alpha: 1.0,
        offset: Vec2::new(0.0, 0.0),
        scale: 1.0,
    };

    /// The ported cascade **entrance** at progress `p`: the letter rises from
    /// [`CASCADE_TRAVEL`] below its slot while fading in.
    ///
    /// Source: `CASCADE_LETTER_VARIANTS`' `initial { opacity: 0, y: "105%" }` to
    /// `animate { opacity: 1, y: "0%" }`.
    pub fn cascade_enter(progress: f64) -> Self {
        CellEffect {
            alpha: progress.clamp(0.0, 1.0),
            offset: Vec2::new(0.0, (1.0 - progress) * CASCADE_TRAVEL),
            scale: 1.0,
        }
    }

    /// The ported cascade **exit** at progress `p`: the letter continues
    /// upward out of its slot while fading out — the opposite direction from
    /// the entrance, which is what makes a swap read as one letter pushing the
    /// other out.
    ///
    /// Source: `CASCADE_LETTER_VARIANTS`' `exit { opacity: 0, y: "-105%" }`.
    pub fn cascade_exit(progress: f64) -> Self {
        CellEffect {
            alpha: 1.0 - progress.clamp(0.0, 1.0),
            offset: Vec2::new(0.0, -progress * CASCADE_TRAVEL),
            scale: 1.0,
        }
    }

    /// The ported cascade in `direction`.
    pub fn cascade(direction: StaggerDirection, progress: f64) -> Self {
        match direction {
            StaggerDirection::Enter => Self::cascade_enter(progress),
            StaggerDirection::Exit => Self::cascade_exit(progress),
        }
    }

    /// A reveal: the letter rises `travel` (a fraction of its own height) into
    /// place while fading in.
    ///
    /// Source: `TextReveal`'s `yOffset` default of `"40%"`
    /// (`components/motion/text-reveal.tsx`) — pass `0.4` for the ported look.
    pub fn reveal(progress: f64, travel: f64) -> Self {
        CellEffect {
            alpha: progress.clamp(0.0, 1.0),
            offset: Vec2::new(0.0, (1.0 - progress) * travel),
            scale: 1.0,
        }
    }
}

impl Default for CellEffect {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// The per-cell staging function a [`CharCellsView`] applies: one cell's raw
/// [`Stagger`] progress in, its [`CellEffect`] out.
type EffectFn = Rc<dyn Fn(f64) -> CellEffect>;

/// A string rendered as independently animated per-grapheme cells.
///
/// Plays its [`Stagger`] once from the frame it first paints, and again whenever
/// the string changes — upstream's own trigger ("changing it cascades the
/// letters to the new value").
///
/// # Example
///
/// ```
/// use frust_beui::motion::{char_cascade, CellEffect};
///
/// // The ported cascade, as-is.
/// let heading = char_cascade::<()>("Ship it");
///
/// // Or the same cells under a caller's own staging.
/// let custom = char_cascade::<()>("Ship it")
///     .effect(|p| CellEffect::reveal(p, 0.4))
///     .clip_to_line(true);
/// ```
pub struct CharCellsView<State: 'static> {
    cells: CharCells,
    views: Vec<AnyView<State>>,
    style: TextStyle,
    /// Whether [`Self::style`] named the family. While `false`, the cells take
    /// `family_role`'s family from the theme at layout.
    family_explicit: bool,
    /// The type-scale role the cells' family resolves from.
    family_role: ThemeTextType,
    color: Option<Color>,
    stagger: Stagger,
    effect: Option<EffectFn>,
    clip_to_line: bool,
}

/// A string cascading in letter by letter — the catalog's default text effect,
/// on upstream's own timing ([`CASCADE_STAGGER`] apart, each letter on
/// [`SPRING_SWAP`](crate::tokens::motion::SPRING_SWAP)).
pub fn char_cascade<State: 'static>(content: impl AsRef<str>) -> CharCellsView<State> {
    let mut view = CharCellsView {
        cells: CharCells::split(content.as_ref()),
        views: Vec::new(),
        style: TextStyle::default(),
        family_explicit: false,
        family_role: CHAR_CELLS_ROLE,
        color: None,
        stagger: Stagger::sprung(CASCADE_STAGGER, crate::tokens::motion::SPRING_SWAP),
        effect: None,
        clip_to_line: false,
    };
    view.rebuild_views();
    view
}

impl<State: 'static> CharCellsView<State> {
    /// Style every cell. The style's family is taken as explicit and wins over
    /// the theme's, whatever order the builders run in. To keep the themed
    /// family, set only the size and colour.
    pub fn style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self.family_explicit = true;
        self.rebuild_views();
        self
    }

    /// Resolve the cells' family from the theme's `role` rather than
    /// [`CHAR_CELLS_ROLE`]. Ignored once [`Self::style`] has named a family.
    pub fn themed_family(mut self, role: ThemeTextType) -> Self {
        self.family_role = role;
        self.rebuild_views();
        self
    }

    /// Set every cell's type size, in logical px.
    pub fn size(mut self, size: f32) -> Self {
        self.style.size = size;
        self.rebuild_views();
        self
    }

    /// Set every cell's colour. Unset, the cells take the theme's default text
    /// colour like any other run.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self.rebuild_views();
        self
    }

    /// Drive the cells with `stagger` instead of the ported cascade timing.
    pub fn stagger(mut self, stagger: Stagger) -> Self {
        self.stagger = stagger;
        self
    }

    /// Shape each cell with `effect` instead of the ported cascade staging.
    ///
    /// The argument is the cell's **raw** stagger progress, so a spring's
    /// overshoot is visible to the effect and can be applied to offset or scale
    /// while opacity clamps it away.
    pub fn effect(mut self, effect: impl Fn(f64) -> CellEffect + 'static) -> Self {
        self.effect = Some(Rc::new(effect));
        self
    }

    /// Clip the cells to the widget's own line box, so a letter travelling out
    /// of its slot disappears at the edge instead of overlapping what is above
    /// or below — the slot-roll look upstream gets from `overflow: hidden`.
    ///
    /// Off by default, because a fade-and-rise reveal wants its letters visible
    /// the whole way.
    pub fn clip_to_line(mut self, clip: bool) -> Self {
        self.clip_to_line = clip;
        self
    }

    /// The cells this view will render.
    pub fn cells(&self) -> &CharCells {
        &self.cells
    }

    /// Rebuild the per-cell text leaves after a change to the content or its
    /// styling.
    fn rebuild_views(&mut self) {
        self.views = self
            .cells
            .cells()
            .iter()
            .map(|cell| {
                let mut leaf = if self.family_explicit {
                    text(cell.text()).style(self.style.clone())
                } else {
                    themed_leaf(cell.text(), &self.style, self.family_role)
                };
                if let Some(color) = self.color {
                    leaf = leaf.color(color);
                }
                any(leaf)
            })
            .collect();
    }
}

/// A cell leaf with every field of `style` except its family, which the leaf
/// resolves from `role` at layout. `.style()` would mark the family explicit,
/// so the fields are set one by one. The ink is set explicitly, which is what
/// `.style()` did. Only `.style()` can set an oblique style, and it takes the
/// explicit path, so upright and italic are all this needs to carry.
fn themed_leaf(content: &str, style: &TextStyle, role: ThemeTextType) -> TextView {
    let leaf = text(content)
        .size(style.size)
        .color(style.color)
        .weight(style.weight)
        .letter_spacing(style.letter_spacing)
        .line_height(style.line_height)
        .align(style.align)
        .themed_family(role);
    if style.style == FontStyle::Italic {
        leaf.italic()
    } else {
        leaf
    }
}

/// The retained widget for a [`CharCellsView`].
pub struct CharCellsWidget {
    cells: Vec<ChildPod>,
    /// The whole string, published as one accessibility node — see
    /// [`CharCellsWidget::semantics`].
    label: String,
    stagger: Stagger,
    effect: Option<EffectFn>,
    clip_to_line: bool,
    /// The frame the current run started at, latched on its first paint so the
    /// run is timed from when it was first seen rather than from the rebuild
    /// that staged it.
    started: Option<FrameTime>,
}

impl CharCellsWidget {
    /// The string this widget renders.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// How many cells it renders it as.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
}

impl<State: 'static> View<State> for CharCellsView<State> {
    type Element = CharCellsWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CharCellsWidget {
        CharCellsWidget {
            cells: self
                .views
                .iter()
                .map(|view| build_child(view, ctx))
                .collect(),
            label: self.cells.source().to_string(),
            stagger: self.stagger,
            effect: self.effect.clone(),
            clip_to_line: self.clip_to_line,
            started: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CharCellsWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.views,
            &self.views,
            &mut element.cells,
            ctx,
            |view| view,
            |_| None,
        );

        if element.label != self.cells.source() {
            // A changed string is a fresh run: upstream's own trigger.
            element.label = self.cells.source().to_string();
            element.started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.stagger != self.stagger {
            element.stagger = self.stagger;
            element.started = None;
            flags |= ChangeFlags::PAINT;
        }
        if element.clip_to_line != self.clip_to_line {
            element.clip_to_line = self.clip_to_line;
            flags |= ChangeFlags::PAINT;
        }
        // Closures are not comparable, so the staging function is reinstalled
        // unconditionally — the same thing every callback-carrying widget does.
        element.effect = self.effect.clone();
        flags
    }

    fn teardown(&self, element: &mut CharCellsWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.views.iter().zip(element.cells.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for CharCellsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let cell_constraints = BoxConstraints::loose(bc.max());
        let mut x = 0.0_f64;
        let mut height = 0.0_f64;
        for pod in &mut self.cells {
            let size = pod.layout_child(ctx, &cell_constraints);
            pod.set_origin(Point::new(x, 0.0));
            x += size.width;
            height = height.max(size.height);
        }
        bc.constrain(Size::new(x, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let count = self.cells.len();
        if count == 0 {
            return;
        }

        // Under `reduce_motion` the cells paint at rest and the widget asks for
        // no frames at all: a decorative letter effect has no position cue worth
        // preserving mid-flight, so freezing it at its settled state is the
        // whole collapse.
        let reduce = Theme::from_paint_ctx(ctx).is_some_and(|theme| theme.motion.reduce_motion);
        let elapsed = if reduce {
            self.stagger.total_duration(count)
        } else {
            let now = ctx.frame_time();
            let started = *self.started.get_or_insert(now);
            now.saturating_sub(started)
        };

        let origin = ctx.origin();
        let line = ctx.size();
        if self.clip_to_line {
            scene.push_clip(origin, line);
        }

        for (index, pod) in self.cells.iter_mut().enumerate() {
            let progress = self.stagger.progress(elapsed, index, count);
            let effect = match &self.effect {
                Some(effect) => effect(progress),
                None => CellEffect::cascade(self.stagger.stagger_direction(), progress),
            };

            let cell_origin = origin + pod.origin().to_vec2();
            let cell = pod.size();
            let translation =
                Vec2::new(effect.offset.x * cell.width, effect.offset.y * cell.height);
            let centre = cell_origin + Vec2::new(cell.width / 2.0, cell.height / 2.0);

            // The layer goes on first so its bounds are stated in the untransformed
            // space the cell was laid out in; the transform then moves the cell
            // within them.
            let bleed = line.height * CELL_LAYER_BLEED;
            scene.push_layer(
                Point::new(cell_origin.x - bleed, cell_origin.y - bleed),
                Size::new(cell.width + bleed * 2.0, cell.height + bleed * 2.0),
                effect.alpha.clamp(0.0, 1.0) as f32,
            );
            scene.push_transform(
                Affine::translate(translation)
                    * Affine::translate(centre.to_vec2())
                    * Affine::scale(effect.scale)
                    * Affine::translate(-centre.to_vec2()),
            );
            pod.paint_child(ctx, scene);
            scene.pop_transform();
            scene.pop_layer();
        }

        if self.clip_to_line {
            scene.pop_clip();
        }

        // A run with a visible endpoint: unpaced frames while it plays, none
        // once it settles.
        if !reduce && !self.stagger.is_settled(elapsed, count) {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Non-interactive: the cells take no input. A broadcast still reaches
        // every child unconditionally, which is what keeps their pods live.
        if event.is_broadcast() {
            for pod in &mut self.cells {
                pod.event_child(ctx, event);
            }
        }
        EventResult::Ignored
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The whole string as one node. Handing a screen reader the letters
        // individually would announce a heading one character at a time, so the
        // cells are forwarded *inside* a labelled container rather than as
        // siblings of it: the container carries the readable value, and no
        // child's subtree is silently dropped.
        ctx.push_container(
            Role::Label,
            |node| node.set_value(self.label.as_str()),
            |ctx| {
                for pod in &self.cells {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    visit_children!(cells);
}

/// U+200D, which binds the cluster on either side of it.
const ZERO_WIDTH_JOINER: char = '\u{200D}';

/// Whether `c` is one of the regional-indicator symbols flags are built from.
fn is_regional_indicator(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

/// Whether `c` attaches to the cluster before it — the documented subset
/// [`CharCells::split`] describes.
fn is_extending(c: char) -> bool {
    matches!(
        c as u32,
        // Combining Diacritical Marks, and its extension/supplement blocks.
        0x0300..=0x036F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        // Cyrillic combining marks.
        | 0x0483..=0x0489
        // Hebrew points and accents.
        | 0x0591..=0x05BD
        | 0x05BF
        | 0x05C1..=0x05C2
        | 0x05C4..=0x05C5
        | 0x05C7
        // Arabic marks.
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x06E7..=0x06E8
        | 0x06EA..=0x06ED
        // Thai vowel signs and tone marks.
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x0E47..=0x0E4E
        // Zero-width joiner, and combining marks for symbols (U+20E3 keycap).
        | 0x200D
        | 0x20D0..=0x20F0
        // Variation selectors, combining half marks, and the supplement.
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
        | 0xE0100..=0xE01EF
        // Tag characters, for emoji subdivision-flag sequences.
        | 0xE0020..=0xE007F
        // Emoji skin-tone modifiers.
        | 0x1F3FB..=0x1F3FF
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust_core::{BuildCtx, PaintCtx};
    use std::any::Any;

    /// The line box every paint test lays its cells into.
    const LINE: Size = Size::new(200.0, 24.0);

    /// A minimal recording scene: the per-cell alpha each composited layer was
    /// pushed at, in paint order. `frust-core`'s own paint-assertion pattern.
    #[derive(Default)]
    struct Recorder {
        alphas: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.alphas.push(alpha);
        }
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
    }

    /// Build `view` into its widget and lay it out into [`LINE`].
    fn laid_out(view: &CharCellsView<()>) -> CharCellsWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(LINE));
        widget
    }

    /// Paint `widget` at `ms` on a synthetic clock, returning the recorded
    /// alphas and whether it asked for another frame.
    fn painted(widget: &mut CharCellsWidget, ms: u64, theme: Option<&Theme>) -> (Vec<f32>, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, LINE, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder.alphas, ctx.needs_frame())
    }

    fn texts(cells: &CharCells) -> Vec<&str> {
        cells.cells().iter().map(CharCell::text).collect()
    }

    /// Plain ASCII splits one cell per character, and the byte ranges index back
    /// into the source.
    #[test]
    fn ascii_splits_one_cell_per_character() {
        let cells = CharCells::split("Ship");
        assert_eq!(cells.len(), 4);
        assert_eq!(texts(&cells), ["S", "h", "i", "p"]);
        for cell in cells.cells() {
            assert_eq!(&cells.source()[cell.range()], cell.text());
        }
    }

    /// A multi-byte character is one cell, not one per byte — the split runs on
    /// characters, never bytes.
    #[test]
    fn a_multi_byte_character_is_one_cell() {
        // Precomposed é, a Japanese syllable, and a two-byte cyrillic letter.
        let cells = CharCells::split("é日ж");
        assert_eq!(cells.len(), 3);
        assert_eq!(texts(&cells), ["é", "日", "ж"]);
        assert_eq!(cells.source().len(), 7, "7 bytes, 3 cells");
    }

    /// A combining sequence stays with its base letter, so an accent never
    /// animates away from the letter it belongs to. This is the case that makes
    /// a bare `chars()` split wrong.
    #[test]
    fn a_combining_mark_stays_with_its_base() {
        // "cafe" + U+0301 COMBINING ACUTE ACCENT.
        let cells = CharCells::split("cafe\u{0301}");
        assert_eq!(
            cells.len(),
            4,
            "four letters, not five: {:?}",
            texts(&cells)
        );
        assert_eq!(cells.cells()[3].text(), "e\u{0301}");

        // Several marks stack onto one base.
        let stacked = CharCells::split("a\u{0300}\u{0301}\u{0302}");
        assert_eq!(stacked.len(), 1);
        assert_eq!(stacked.cells()[0].text(), "a\u{0300}\u{0301}\u{0302}");
    }

    /// Emoji sequences hold together: a flag is its two regional indicators, a
    /// skin-toned emoji is its base plus modifier, and a joined sequence is one
    /// cell however many parts it has.
    #[test]
    fn emoji_sequences_stay_whole() {
        let flag = CharCells::split("\u{1F1EC}\u{1F1E7}");
        assert_eq!(flag.len(), 1, "one flag: {:?}", texts(&flag));

        // Two flags in a row split between them, not down the middle.
        let flags = CharCells::split("\u{1F1EC}\u{1F1E7}\u{1F1EB}\u{1F1F7}");
        assert_eq!(flags.len(), 2);
        assert_eq!(flags.cells()[1].text(), "\u{1F1EB}\u{1F1F7}");

        let toned = CharCells::split("\u{1F44D}\u{1F3FD}");
        assert_eq!(toned.len(), 1, "thumbs-up plus skin tone is one cell");

        // Woman + ZWJ + laptop: one professional emoji.
        let joined = CharCells::split("\u{1F469}\u{200D}\u{1F4BB}");
        assert_eq!(joined.len(), 1, "a joined sequence is one cell");

        // A variation selector attaches rather than opening a cell of its own.
        let varied = CharCells::split("\u{2764}\u{FE0F}");
        assert_eq!(varied.len(), 1);
    }

    /// An odd regional indicator is a cell of its own rather than swallowing the
    /// letter after it.
    #[test]
    fn a_lone_regional_indicator_does_not_absorb_what_follows() {
        let cells = CharCells::split("\u{1F1EC}a");
        assert_eq!(cells.len(), 2);
        assert_eq!(texts(&cells), ["\u{1F1EC}", "a"]);
    }

    /// Whitespace is a cell like any other — a cascade has to be able to leave a
    /// gap where a space is, and the source round-trips exactly.
    #[test]
    fn whitespace_is_preserved_as_its_own_cell() {
        let cells = CharCells::split("a b");
        assert_eq!(texts(&cells), ["a", " ", "b"]);
        let rejoined: String = cells.cells().iter().map(CharCell::text).collect();
        assert_eq!(rejoined, cells.source());

        // CRLF is one cell, not two.
        let crlf = CharCells::split("a\r\nb");
        assert_eq!(crlf.len(), 3);
        assert_eq!(crlf.cells()[1].text(), "\r\n");
    }

    /// An empty string splits into nothing and reports so.
    #[test]
    fn an_empty_string_has_no_cells() {
        let cells = CharCells::split("");
        assert!(cells.is_empty());
        assert_eq!(cells.len(), 0);
        assert_eq!(cells.source(), "");
    }

    /// Whatever the input, the cells reassemble into it exactly — the invariant
    /// that makes a cell split safe to render in place of the original run.
    #[test]
    fn the_cells_always_reassemble_into_the_source() {
        for source in [
            "Ship it",
            "cafe\u{0301} \u{1F1EC}\u{1F1E7}",
            "\u{1F469}\u{200D}\u{1F4BB}!",
            "\u{0E01}\u{0E31}\u{0E19}",
            "",
            " ",
        ] {
            let cells = CharCells::split(source);
            let rejoined: String = cells.cells().iter().map(CharCell::text).collect();
            assert_eq!(rejoined, source, "round trip failed for {source:?}");
        }
    }

    /// The ported cascade staging: an entrance rises from below into place while
    /// fading in, an exit continues upward while fading out.
    #[test]
    fn the_cascade_staging_matches_the_ported_variants() {
        let start = CellEffect::cascade_enter(0.0);
        assert_eq!(start.alpha, 0.0);
        assert_eq!(start.offset.y, CASCADE_TRAVEL, "starts a full slot below");

        let landed = CellEffect::cascade_enter(1.0);
        assert_eq!(landed, CellEffect::IDENTITY);

        let leaving = CellEffect::cascade_exit(1.0);
        assert_eq!(leaving.alpha, 0.0);
        assert_eq!(leaving.offset.y, -CASCADE_TRAVEL, "exits the opposite way");
        assert_eq!(CellEffect::cascade_exit(0.0), CellEffect::IDENTITY);

        assert_eq!(
            CellEffect::cascade(StaggerDirection::Exit, 0.5),
            CellEffect::cascade_exit(0.5)
        );
    }

    /// A spring's overshoot reaches offset and scale while opacity stays bounded
    /// — the raw/clamped split, seen from the staging side.
    #[test]
    fn an_overshooting_progress_moves_the_cell_without_over_brightening_it() {
        let overshot = CellEffect::cascade_enter(1.2);
        assert_eq!(overshot.alpha, 1.0, "opacity clamps");
        assert!(overshot.offset.y < 0.0, "position overshoots past its slot");
    }

    /// The reveal staging carries its own travel distance.
    #[test]
    fn the_reveal_staging_rises_by_its_own_travel() {
        assert_eq!(CellEffect::reveal(0.0, 0.4).offset.y, 0.4);
        assert_eq!(CellEffect::reveal(1.0, 0.4), CellEffect::IDENTITY);
    }

    /// The view splits its content into the cells it will render, and the
    /// builders leave that split alone.
    #[test]
    fn the_view_renders_one_cell_per_grapheme() {
        let view = char_cascade::<()>("cafe\u{0301}!");
        assert_eq!(view.cells().len(), 5);

        let styled = char_cascade::<()>("cafe\u{0301}!")
            .size(24.0)
            .color(Color::from_rgb8(1, 2, 3))
            .clip_to_line(true);
        assert_eq!(styled.cells().len(), 5);
        assert_eq!(styled.cells().source(), "cafe\u{0301}!");
    }

    /// The whole point of the substrate, seen through paint: at one instant
    /// mid-run each letter is composited at its own opacity, earlier letters
    /// ahead of later ones, and the widget keeps asking for frames until the
    /// last one has landed.
    #[test]
    fn the_cells_composite_in_stagger_order_until_the_run_settles() {
        // The timing `char_cascade` documents itself as defaulting to.
        let stagger = Stagger::sprung(CASCADE_STAGGER, crate::tokens::motion::SPRING_SWAP);
        let mut widget = laid_out(&char_cascade::<()>("Ship"));

        // First paint latches the clock; nothing has moved yet.
        let (start, needs_frame) = painted(&mut widget, 1_000, None);
        assert_eq!(start.len(), 4, "one composited layer per cell");
        assert!(start.iter().all(|alpha| *alpha == 0.0));
        assert!(needs_frame, "a run with frames left to draw asks for them");

        // Mid-run: strictly ordered, first letter furthest along.
        let (mid, needs_frame) = painted(&mut widget, 1_000 + 40, None);
        assert!(needs_frame);
        for pair in mid.windows(2) {
            assert!(
                pair[0] >= pair[1],
                "letters must not overtake each other: {mid:?}"
            );
        }
        assert!(mid[0] > mid[3], "and the run is genuinely spread: {mid:?}");

        // Settled: everything at full opacity, and no further frames owed.
        // Rounded up: the timeline is sub-millisecond precise and the clock
        // here is not, so a truncated reading would land a fraction short of the
        // last cell's own settle.
        let total = stagger.total_duration(4).as_millis() as u64 + 1;
        let (settled, needs_frame) = painted(&mut widget, 1_000 + total, None);
        assert!(settled.iter().all(|alpha| *alpha == 1.0), "{settled:?}");
        assert!(!needs_frame, "a settled run stops requesting frames");
    }

    /// Under `reduce_motion` the cells paint at rest on the very first frame and
    /// the widget never asks for another — the collapse, not a faster cascade.
    #[test]
    fn reduce_motion_paints_the_cells_at_rest_and_asks_for_nothing() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;

        let mut widget = laid_out(&char_cascade::<()>("Ship"));
        let (alphas, needs_frame) = painted(&mut widget, 0, Some(&theme));
        assert_eq!(alphas, vec![1.0, 1.0, 1.0, 1.0]);
        assert!(!needs_frame);

        // And it stays that way rather than starting once a clock advances.
        let (later, needs_frame) = painted(&mut widget, 10_000, Some(&theme));
        assert_eq!(later, vec![1.0, 1.0, 1.0, 1.0]);
        assert!(!needs_frame);
    }

    /// An empty string paints nothing at all and asks for no frames, rather than
    /// running an empty timeline forever.
    #[test]
    fn an_empty_string_paints_nothing() {
        let mut widget = laid_out(&char_cascade::<()>(""));
        let (alphas, needs_frame) = painted(&mut widget, 0, None);
        assert!(alphas.is_empty());
        assert!(!needs_frame);
    }

    /// An exit run composites the other way round: the leading letter is the
    /// first to disappear.
    #[test]
    fn an_exit_run_fades_its_leading_letter_out_first() {
        let view = char_cascade::<()>("Ship").stagger(
            Stagger::sprung(CASCADE_STAGGER, crate::tokens::motion::SPRING_SWAP).exiting(),
        );
        let mut widget = laid_out(&view);
        painted(&mut widget, 0, None);
        let (mid, _) = painted(&mut widget, 40, None);
        for pair in mid.windows(2) {
            assert!(pair[0] <= pair[1], "the tail lingers: {mid:?}");
        }
        assert!(
            mid[0] < mid[3],
            "and the head has genuinely gone first: {mid:?}"
        );
    }

    // ---- Typeface: the cells' family follows the live theme ----------------

    use crate::text::typeface_probe::{
        Face, Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// The window every typeface probe paints the cells into.
    const PROBE_WINDOW: Size = Size::new(240.0, 60.0);

    #[test]
    fn the_cells_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist(
            "the cascade's cells",
            |_: &mut ()| char_cascade::<()>("Ship"),
            PROBE_WINDOW,
        );
    }

    #[test]
    fn the_cells_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap(
            "the cascade's cells",
            |_: &mut ()| {
                char_cascade::<()>("Ship")
                    .size(24.0)
                    .color(Color::from_rgb8(1, 2, 3))
            },
            PROBE_WINDOW,
        );
    }

    #[test]
    fn a_named_role_resolves_that_roles_family() {
        // Only title_large carries Geist Mono, so the default role stays Geist
        // and the named one does not.
        let mut theme = crate::theme();
        theme.type_scale.title_large.family = crate::tokens::mono_family();
        let default = Probe::new(
            |_: &mut ()| char_cascade::<()>("Ship"),
            PROBE_WINDOW,
            theme.clone(),
        )
        .frame();
        assert_eq!(default, vec![Face::Geist; 4]);
        let named = Probe::new(
            |_: &mut ()| char_cascade::<()>("Ship").themed_family(ThemeTextType::TitleLarge),
            PROBE_WINDOW,
            theme,
        )
        .frame();
        assert_eq!(named, vec![Face::GeistMono; 4]);
    }

    #[test]
    fn an_explicit_style_family_wins_over_the_theme() {
        // `.style()` names the family, before or after `.themed_family()`.
        let mono = TextStyle {
            family: crate::tokens::mono_family(),
            ..TextStyle::default()
        };
        for view in [
            char_cascade::<()>("Ship")
                .style(mono.clone())
                .themed_family(ThemeTextType::BodyMedium),
            char_cascade::<()>("Ship")
                .themed_family(ThemeTextType::BodyMedium)
                .style(mono.clone()),
        ] {
            let mut view = Some(view);
            let faces = Probe::new(
                move |_: &mut ()| view.take().expect("built once"),
                PROBE_WINDOW,
                crate::theme(),
            )
            .frame();
            assert_eq!(faces, vec![Face::GeistMono; 4]);
        }
    }
}
