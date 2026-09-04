//! Ports beUI's `loader` component.
//!
//! **Source:** `components/motion/loader.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. The
//! registry entry describes it as *"a loading indicator with seventeen
//! variants"*, and all seventeen arrive here as [`LoaderVariant`] arms —
//! [`LoaderVariant::ALL`] is the enumeration seam a catalog page walks.
//!
//! # One size prop, one ink, one clock
//!
//! Upstream's three knobs travel intact: `size` is the base square every metric
//! is derived from ([`LoaderView::size`]), `speed` is seconds per cycle
//! ([`LoaderView::speed`]), and every part paints `currentColor`, which here is
//! one resolved ink ([`LoaderView::color`], else the theme's `on_surface`).
//! Nothing in this component holds per-part animation state: each frame is a
//! pure function of one elapsed time, exactly as a CSS keyframe set is.
//!
//! # Reduced motion keeps the pulse — deliberately
//!
//! This component departs from the catalog's usual reduced-motion collapse
//! (paint the settled state, request no frames), because upstream's own rule is
//! different and the reason is sound: *"reduced motion keeps a calm opacity
//! pulse and drops every transform"*. A loader frozen mid-spin reads as a hung
//! application rather than as a calmed animation, so every variant instead
//! paints its **rest pose** modulated by upstream's `[1, 0.4, 1]` opacity pulse
//! over [`REDUCED_PERIOD`], and asks for a *paceable* cosmetic frame rather than
//! an unpaced one. The glyph-cycling variants keep cycling (a glyph swap is not
//! on-screen movement) at upstream's slowed cadence.
//!
//! # Degradations against upstream
//!
//! - **`metaballs` is not gooey.** Upstream merges two circles with an SVG
//!   `feGaussianBlur` + `feColorMatrix` threshold; frust's scene publishes no
//!   filter primitive, so the merge degrades to an explicit **capsule bridge**
//!   drawn between the two circles, thickening as they close. The silhouette is
//!   the same; the soft blend at the seam is not.
//! - **`morph` interpolates radii, not path data.** Upstream tweens the SVG `d`
//!   attribute point-to-point; the same 24 sampled points are used here, but
//!   the interpolation is over each point's radius, which is the same curve for
//!   these shapes and avoids carrying a path-tweening primitive.
//! - **No blur on `dither`/`dot-matrix`.** Neither upstream variant blurs, so
//!   nothing is lost — noted only because the sibling text effects do.
//! - **The ascii sets need a monospace face with those glyphs.** Braille and
//!   block-drawing frames come from the bundled Geist Mono stack; a host that
//!   overrides the mono family with a face lacking them gets the font stack's
//!   own fallback, not a drawn substitute.

use std::f64::consts::{PI, TAU};
use std::time::Duration;

use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Affine, BezPath, BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Role,
    SemanticsCtx, Shape, TickClass, View, Widget,
};
use frust::{FrameTime, Theme};
use kurbo::{Arc, Point, Size, Vec2};
use peniko::{Brush, Color};

use crate::style::{PATH_TOLERANCE, scale_alpha};
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::EASE_IN_OUT;

/// The base square every metric is derived from — upstream's `size = 32`.
pub const DEFAULT_SIZE: f64 = 32.0;

/// Seconds per animation cycle — upstream's `speed = 1`.
pub const DEFAULT_SPEED: Duration = Duration::from_millis(1000);

/// The accessible name a loader announces — upstream's `label = "Loading"`.
pub const DEFAULT_LABEL: &str = "Loading";

/// Period of the reduced-motion opacity pulse — upstream's `REDUCED`
/// transition, `duration: 1.4`.
pub const REDUCED_PERIOD: Duration = Duration::from_millis(1400);

/// The floor of the reduced-motion pulse — upstream's `opacity: [1, 0.4, 1]`.
const REDUCED_FLOOR: f32 = 0.4;

/// How much slower a glyph-cycling variant runs under reduced motion —
/// upstream's `speed * 2.5`.
const REDUCED_GLYPH_FACTOR: f64 = 2.5;

/// Alpha of the spinner's static track ring — `strokeOpacity={0.2}`.
const TRACK_ALPHA: f32 = 0.2;

/// The spinner's drawn sweep: three quarters of a turn.
const SPINNER_SWEEP: f64 = PI * 1.5;

/// The scramble variant's fixed target — `SCRAMBLE_TARGET`.
const SCRAMBLE_TARGET: &str = "LOADING";

/// The glyphs the scramble variant samples — `SCRAMBLE_GLYPHS`.
const SCRAMBLE_GLYPHS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<>/*#@";

/// How many points each morph shape is sampled at — `MORPH_POINTS = 24`.
const MORPH_POINTS: usize = 24;

/// The morph's own cycle is five times the base speed — `duration: speed * 5`.
const MORPH_CYCLES: u32 = 5;

/// The ordered Bayer 4×4 threshold matrix the dither variant lights cells in —
/// `BAYER_4`, transcribed verbatim.
const BAYER_4: [usize; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// The newton variant's ball count — `NEWTON_BALLS`.
const NEWTON_BALLS: usize = 5;

/// The comet variant's trail length — `COMET_TRAIL`.
const COMET_TRAIL: usize = 6;

/// The helix variant's row count.
const HELIX_ROWS: usize = 7;

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.foreground;

/// Which of the seventeen upstream loaders a [`LoaderView`] draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoaderVariant {
    /// A rotating partial ring over a static track.
    #[default]
    Spinner,
    /// Three bouncing dots.
    Dots,
    /// Four bars pumping from the baseline.
    Bars,
    /// A 3×3 grid lighting on a diagonal wave.
    DotMatrix,
    /// A 4×4 grid lighting in Bayer order — a dissolving halftone.
    Dither,
    /// The braille spinner CLI agents cycle.
    Ascii,
    /// The `|/-\` line spinner.
    AsciiLine,
    /// The eight-frame braille spinner.
    AsciiBraille,
    /// The block-height bounce.
    AsciiBlocks,
    /// The single-dot bounce.
    AsciiBounce,
    /// A polygon morphing circle → square → triangle → hexagon → diamond.
    Morph,
    /// A rotating head with a tapering trail.
    Comet,
    /// The word `LOADING` resolving out of random glyphs, forever.
    Scramble,
    /// Two circles passing through each other.
    Metaballs,
    /// A newton's cradle: only the end balls move.
    Newton,
    /// Two counter-phased dot columns, read as a double helix.
    Helix,
    /// A counting percentage over a progress bar.
    Percent,
}

impl LoaderVariant {
    /// Every variant, in `LoaderVariant`'s own upstream declaration order — the
    /// seam a catalog page or a test enumerates the set through.
    pub const ALL: [LoaderVariant; 17] = [
        LoaderVariant::Spinner,
        LoaderVariant::Dots,
        LoaderVariant::Bars,
        LoaderVariant::DotMatrix,
        LoaderVariant::Dither,
        LoaderVariant::Ascii,
        LoaderVariant::AsciiLine,
        LoaderVariant::AsciiBraille,
        LoaderVariant::AsciiBlocks,
        LoaderVariant::AsciiBounce,
        LoaderVariant::Morph,
        LoaderVariant::Comet,
        LoaderVariant::Scramble,
        LoaderVariant::Metaballs,
        LoaderVariant::Newton,
        LoaderVariant::Helix,
        LoaderVariant::Percent,
    ];

    /// The frame set a glyph-cycling variant walks, or `None` for a drawn one —
    /// upstream's `ASCII_SETS` lookup.
    pub fn ascii_frames(self) -> Option<&'static [&'static str]> {
        Some(match self {
            LoaderVariant::Ascii => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            LoaderVariant::AsciiLine => &["|", "/", "-", "\\"],
            LoaderVariant::AsciiBraille => &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"],
            LoaderVariant::AsciiBlocks => &[
                "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂",
            ],
            LoaderVariant::AsciiBounce => &["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"],
            _ => return None,
        })
    }

    /// Whether this variant paints shaped glyphs rather than geometry.
    pub fn is_textual(self) -> bool {
        self.ascii_frames().is_some()
            || matches!(self, LoaderVariant::Scramble | LoaderVariant::Percent)
    }

    /// The strings this variant needs shaped, in the order `paint` indexes them.
    fn glyph_keys(self) -> Vec<String> {
        if let Some(frames) = self.ascii_frames() {
            return frames.iter().map(|frame| (*frame).to_string()).collect();
        }
        match self {
            // The sampled alphabet first, then the target's own letters, so a
            // resolved position indexes past the alphabet.
            LoaderVariant::Scramble => SCRAMBLE_GLYPHS
                .chars()
                .chain(SCRAMBLE_TARGET.chars())
                .map(|c| c.to_string())
                .collect(),
            LoaderVariant::Percent => (0..10)
                .map(|digit| digit.to_string())
                .chain(std::iter::once("%".to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The type size this variant's glyphs are shaped at, relative to the base
    /// square.
    fn glyph_size(self, size: f64) -> f64 {
        match self {
            LoaderVariant::Scramble | LoaderVariant::Percent => size * 0.42,
            _ => size,
        }
    }

    /// The box this variant occupies at base square `size`, before constraints.
    fn natural_size(self, size: f64) -> Size {
        match self {
            LoaderVariant::Dots => {
                let dot = size * 0.24;
                Size::new(dot * 3.0 + size * 0.14 * 2.0, dot)
            }
            LoaderVariant::Bars => Size::new(size * 0.16 * 4.0 + size * 0.1 * 3.0, size),
            LoaderVariant::Newton => {
                let ball = size * 0.2;
                Size::new(ball * NEWTON_BALLS as f64, ball)
            }
            // A counting percentage sits over a bar wider than the square —
            // `width: size * 1.4`.
            LoaderVariant::Percent => Size::new(size * 1.4, size),
            _ => Size::new(size, size),
        }
    }
}

/// A declarative beUI loading indicator. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::components::loader::{LoaderVariant, loader};
///
/// let busy = loader::<()>().variant(LoaderVariant::Dots).size(24.0);
/// ```
pub struct LoaderView<State: 'static> {
    variant: LoaderVariant,
    size: f64,
    speed: Duration,
    label: String,
    color: Option<Color>,
    _state: std::marker::PhantomData<fn(&mut State)>,
}

/// Create a loader at upstream's defaults: a [`Spinner`](LoaderVariant::Spinner)
/// of [`DEFAULT_SIZE`] running at [`DEFAULT_SPEED`].
pub fn loader<State: 'static>() -> LoaderView<State> {
    LoaderView {
        variant: LoaderVariant::default(),
        size: DEFAULT_SIZE,
        speed: DEFAULT_SPEED,
        label: DEFAULT_LABEL.to_string(),
        color: None,
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> LoaderView<State> {
    /// Draw `variant` instead of the spinner.
    pub fn variant(mut self, variant: LoaderVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Set the base square, in logical px (default [`DEFAULT_SIZE`]). Every
    /// metric scales from it.
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0);
        self
    }

    /// Set one animation cycle's length (default [`DEFAULT_SPEED`]).
    pub fn speed(mut self, speed: Duration) -> Self {
        self.speed = speed;
        self
    }

    /// Set the accessible name (default [`DEFAULT_LABEL`]).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Paint in `color` instead of the theme's resolved ink — upstream's
    /// `currentColor`, which frust has no inheritance channel for.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

/// The retained widget for a [`LoaderView`].
pub struct LoaderWidget {
    variant: LoaderVariant,
    size: f64,
    speed: Duration,
    label: String,
    color: Option<Color>,
    /// The shaped glyphs a textual variant indexes, in
    /// [`LoaderVariant::glyph_keys`] order.
    glyphs: Vec<TextLayout>,
    shaped: Option<(LoaderVariant, TextStyle)>,
    /// The frame this loader started at, latched on its first paint.
    started: Option<FrameTime>,
}

impl LoaderWidget {
    /// Which loader this is.
    pub fn variant(&self) -> LoaderVariant {
        self.variant
    }

    /// Its accessible name.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The resolved ink: the explicit override, else the theme's `on_surface`,
    /// else [`FALLBACK_INK`].
    fn ink(&self, theme: Option<&Theme>) -> Color {
        self.color
            .unwrap_or_else(|| theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface))
    }

    /// The style a textual variant's glyphs are shaped with — beUI's mono stack,
    /// which is what upstream's `font-mono` resolves to.
    fn glyph_style(&self, theme: Option<&Theme>, ink: Color) -> TextStyle {
        let _ = theme;
        TextStyle {
            family: crate::tokens::mono_family(),
            ..TextStyle::new(self.variant.glyph_size(self.size) as f32, ink)
        }
    }

    /// Which ascii frame is showing at `elapsed`.
    fn ascii_index(&self, elapsed: Duration, reduce: bool) -> usize {
        let Some(frames) = self.variant.ascii_frames() else {
            return 0;
        };
        let period = if reduce {
            self.speed.mul_f64(REDUCED_GLYPH_FACTOR)
        } else {
            self.speed
        };
        let span = period.as_nanos();
        if span == 0 || frames.is_empty() {
            return 0;
        }
        // Integer arithmetic, not a float ratio: a frame boundary landing one
        // ulp early would show the previous glyph for a whole tick.
        let count = frames.len() as u128;
        ((elapsed.as_nanos() * count / span) % count) as usize
    }

    /// The scramble's sampled string at `elapsed` — the same left-to-right
    /// resolve upstream ticks, cycling forever.
    fn scramble_text(&self, elapsed: Duration, reduce: bool) -> String {
        if reduce {
            return SCRAMBLE_TARGET.to_string();
        }
        let letters = SCRAMBLE_TARGET.len();
        // `(speed / length) * 0.55` seconds per tick, `length + 4` ticks a cycle.
        let tick = self.speed.mul_f64(0.55 / letters as f64);
        if tick.is_zero() {
            return SCRAMBLE_TARGET.to_string();
        }
        let total = letters + 4;
        let ticks = (elapsed.as_secs_f64() / tick.as_secs_f64()).floor() as i64;
        let reveal = ticks.rem_euclid(total as i64) as usize;
        let alphabet: Vec<char> = SCRAMBLE_GLYPHS.chars().collect();
        SCRAMBLE_TARGET
            .chars()
            .enumerate()
            .map(|(index, letter)| {
                if index < reveal {
                    letter
                } else {
                    alphabet[(mix(ticks as u64, index as u64) % alphabet.len() as u64) as usize]
                }
            })
            .collect()
    }

    /// The percentage showing at `elapsed`, `0..=100`.
    fn percent(&self, elapsed: Duration, reduce: bool) -> u32 {
        let period = if reduce {
            self.speed.mul_f64(2.0)
        } else {
            self.speed
        };
        if period.is_zero() {
            return 100;
        }
        let phase = fract(elapsed.as_secs_f64() / period.as_secs_f64());
        ((phase * 100.0).round() as u32).min(100)
    }
}

impl<State: 'static> View<State> for LoaderView<State> {
    type Element = LoaderWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LoaderWidget {
        LoaderWidget {
            variant: self.variant,
            size: self.size,
            speed: self.speed,
            label: self.label.clone(),
            color: self.color,
            glyphs: Vec::new(),
            shaped: None,
            started: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut LoaderWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.variant != self.variant {
            element.variant = self.variant;
            element.shaped = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            element.shaped = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.speed != self.speed {
            element.speed = self.speed;
            flags |= ChangeFlags::PAINT;
        }
        if prev.label != self.label {
            element.label = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }
        if prev.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for LoaderWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if self.variant.is_textual() {
            let theme = Theme::from_layout_ctx(ctx);
            let style = self.glyph_style(theme, self.ink(theme));
            if self.shaped.as_ref() != Some(&(self.variant, style.clone())) {
                let keys = self.variant.glyph_keys();
                let text_ctx = ctx.text_context::<TextContext>();
                self.glyphs = keys
                    .iter()
                    .map(|key| text_ctx.layout(key, &style, None))
                    .collect();
                self.shaped = Some((self.variant, style));
            }
        }
        bc.constrain(self.variant.natural_size(self.size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);

        // Under reduced motion every transform is dropped and the whole
        // indicator breathes instead — see the module docs.
        let ink = if reduce {
            scale_alpha(self.ink(theme), reduced_pulse(elapsed))
        } else {
            self.ink(theme)
        };
        // The drawn box is the variant's own, centred in whatever the parent
        // gave it.
        let natural = self.variant.natural_size(self.size);
        let box_size = ctx.size();
        let origin = Point::new(
            ctx.origin().x + (box_size.width - natural.width).max(0.0) / 2.0,
            ctx.origin().y + (box_size.height - natural.height).max(0.0) / 2.0,
        );

        self.paint_variant(origin, natural, elapsed, reduce, ink, scene);

        // A loader is a perpetual decorative loop, never a transition with an
        // endpoint — so the mobile frame gate may pace it.
        ctx.request_frame_class(TickClass::CosmeticLoop);
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="status" aria-label={label}` upstream.
        ctx.push_node(Role::Status, |node| {
            node.set_label(self.label.as_str());
        });
    }
}

impl LoaderWidget {
    /// Draw the current variant into `origin`/`size`.
    fn paint_variant(
        &self,
        origin: Point,
        size: Size,
        elapsed: Duration,
        reduce: bool,
        ink: Color,
        scene: &mut dyn PaintScene,
    ) {
        let base = self.size;
        let period = self.speed;
        match self.variant {
            LoaderVariant::Spinner => {
                let stroke = (base * 0.09).max(2.0);
                let radius = ((base - stroke) / 2.0).max(0.0);
                let centre = Point::new(origin.x + base / 2.0, origin.y + base / 2.0);
                let track = Arc::new(centre, Vec2::new(radius, radius), 0.0, TAU, 0.0);
                scene.stroke_path(
                    Point::ORIGIN,
                    &track.to_path(PATH_TOLERANCE),
                    stroke,
                    &Brush::Solid(scale_alpha(ink, TRACK_ALPHA)),
                );
                // Rest pose is the arc at zero rotation; reduced motion holds it
                // there and lets the pulse carry the life.
                let angle = if reduce {
                    0.0
                } else {
                    phase(elapsed, period, Duration::ZERO) * TAU
                };
                let head = Arc::new(centre, Vec2::new(radius, radius), angle, SPINNER_SWEEP, 0.0);
                scene.stroke_path(
                    Point::ORIGIN,
                    &head.to_path(PATH_TOLERANCE),
                    stroke,
                    &Brush::Solid(ink),
                );
            }
            LoaderVariant::Dots => {
                let dot = base * 0.24;
                let gap = base * 0.14;
                for index in 0..3 {
                    let x = origin.x + index as f64 * (dot + gap) + dot / 2.0;
                    let delay = period.mul_f64(0.16 * index as f64);
                    let (rise, alpha) = if reduce {
                        (0.0, 1.0)
                    } else {
                        let wave = ping_pong(phase(elapsed, period, delay));
                        (wave * base * 0.3, 0.5 + wave * 0.5)
                    };
                    circle(
                        scene,
                        Point::new(x, origin.y + dot / 2.0 - rise),
                        dot,
                        scale_alpha(ink, alpha as f32),
                    );
                }
            }
            LoaderVariant::Bars => {
                let bar = base * 0.16;
                let gap = base * 0.1;
                for index in 0..4 {
                    let delay = period.mul_f64(0.12 * index as f64);
                    // `originY: 1` — the bar pumps from its baseline.
                    let scale = if reduce {
                        1.0
                    } else {
                        0.3 + ping_pong(phase(elapsed, period, delay)) * 0.7
                    };
                    let height = base * scale;
                    scene.fill_rounded_rect(
                        Point::new(
                            origin.x + index as f64 * (bar + gap),
                            origin.y + base - height,
                        ),
                        Size::new(bar, height),
                        bar / 2.0,
                        ink,
                    );
                }
            }
            LoaderVariant::DotMatrix => {
                let cells = 3usize;
                let gap = base * 0.14;
                let dot = (base - gap * (cells - 1) as f64) / cells as f64;
                for index in 0..cells * cells {
                    let (x, y) = (index % cells, index / cells);
                    let delay = period.mul_f64((x + y) as f64 / (2.0 * (cells - 1) as f64));
                    let (alpha, scale) = if reduce {
                        (1.0, 1.0)
                    } else {
                        let wave = ping_pong(phase(elapsed, period, delay));
                        (0.2 + wave * 0.8, 0.7 + wave * 0.3)
                    };
                    let centre = Point::new(
                        origin.x + x as f64 * (dot + gap) + dot / 2.0,
                        origin.y + y as f64 * (dot + gap) + dot / 2.0,
                    );
                    circle(scene, centre, dot * scale, scale_alpha(ink, alpha as f32));
                }
            }
            LoaderVariant::Dither => {
                let cells = 4usize;
                let gap = (base * 0.05).max(1.0);
                let cell = (base - gap * (cells - 1) as f64) / cells as f64;
                for (index, order) in BAYER_4.iter().enumerate() {
                    let (x, y) = (index % cells, index / cells);
                    let delay = period.mul_f64(*order as f64 / BAYER_4.len() as f64);
                    let alpha = if reduce {
                        1.0
                    } else {
                        0.1 + ping_pong(phase(elapsed, period, delay)) * 0.9
                    };
                    scene.fill_rect(
                        Point::new(
                            origin.x + x as f64 * (cell + gap),
                            origin.y + y as f64 * (cell + gap),
                        ),
                        Size::new(cell, cell),
                        scale_alpha(ink, alpha as f32),
                    );
                }
            }
            LoaderVariant::Ascii
            | LoaderVariant::AsciiLine
            | LoaderVariant::AsciiBraille
            | LoaderVariant::AsciiBlocks
            | LoaderVariant::AsciiBounce => {
                let index = self.ascii_index(elapsed, reduce);
                if let Some(run) = self.glyphs.get(index) {
                    let glyph = run.size();
                    draw_run(
                        run,
                        Point::new(
                            origin.x + (size.width - glyph.width) / 2.0,
                            origin.y + (size.height - glyph.height) / 2.0,
                        ),
                        &Brush::Solid(ink),
                        scene,
                    );
                }
            }
            LoaderVariant::Morph => {
                let cycle = period * MORPH_CYCLES;
                let at = if reduce {
                    0.0
                } else {
                    phase(elapsed, cycle, Duration::ZERO)
                };
                let (radii, rotation, scale) = morph_at(at);
                let centre = Point::new(origin.x + base / 2.0, origin.y + base / 2.0);
                let path = morph_path(&radii, centre, base / 2.0 * 0.92 * scale);
                scene.push_transform(rotate_about(rotation, centre));
                scene.fill_path(Point::ORIGIN, &path, &Brush::Solid(ink));
                scene.pop_transform();
            }
            LoaderVariant::Comet => {
                let head = base * 0.2;
                let radius = base / 2.0 - head / 2.0;
                let centre = Point::new(origin.x + base / 2.0, origin.y + base / 2.0);
                let spin = if reduce {
                    0.0
                } else {
                    phase(elapsed, period, Duration::ZERO) * TAU
                };
                for index in 0..COMET_TRAIL {
                    let scale = 1.0 - index as f64 * 0.13;
                    let alpha = 1.0 - index as f64 * 0.16;
                    // Each trail dot sits a fixed angle behind the head, on the
                    // same orbit — `rotate(-i*15deg) translateY(-r)`.
                    let angle = spin - index as f64 * 15.0_f64.to_radians();
                    let at = Point::new(
                        centre.x + angle.sin() * radius,
                        centre.y - angle.cos() * radius,
                    );
                    circle(scene, at, head * scale, scale_alpha(ink, alpha as f32));
                }
            }
            LoaderVariant::Scramble => {
                let text = self.scramble_text(elapsed, reduce);
                let alphabet = SCRAMBLE_GLYPHS.chars().count();
                let cell = self
                    .glyphs
                    .iter()
                    .map(|run| run.size().width)
                    .fold(0.0_f64, f64::max);
                let height = self
                    .glyphs
                    .iter()
                    .map(|run| run.size().height)
                    .fold(0.0_f64, f64::max);
                // A fixed advance per position, so a resolving letter never
                // shuffles the ones beside it.
                let total = cell * text.chars().count() as f64;
                let mut x = origin.x + (size.width - total).max(0.0) / 2.0;
                let y = origin.y + (size.height - height).max(0.0) / 2.0;
                for character in text.chars() {
                    let index = SCRAMBLE_GLYPHS
                        .chars()
                        .position(|c| c == character)
                        .or_else(|| {
                            SCRAMBLE_TARGET
                                .chars()
                                .position(|c| c == character)
                                .map(|i| alphabet + i)
                        });
                    if let Some(run) = index.and_then(|i| self.glyphs.get(i)) {
                        let glyph = run.size();
                        draw_run(
                            run,
                            Point::new(x + (cell - glyph.width) / 2.0, y),
                            &Brush::Solid(ink),
                            scene,
                        );
                    }
                    x += cell;
                }
            }
            LoaderVariant::Metaballs => {
                let radius = base * 0.15;
                let travel = base * 0.2;
                let centre_y = origin.y + base / 2.0;
                let wave = if reduce {
                    0.5
                } else {
                    ping_pong(phase(elapsed, period.mul_f64(1.6), Duration::ZERO))
                };
                let left = Point::new(
                    origin.x + base / 2.0 - travel + wave * travel * 2.0,
                    centre_y,
                );
                let right = Point::new(
                    origin.x + base / 2.0 + travel - wave * travel * 2.0,
                    centre_y,
                );
                // The gooey merge upstream gets from a blur/threshold filter is
                // drawn explicitly here — see the module docs.
                let gap = (right.x - left.x).abs();
                let bridge = (1.0 - gap / (radius * 4.0)).clamp(0.0, 1.0) * radius;
                if bridge > 0.0 && gap > 0.0 {
                    scene.fill_rounded_rect(
                        Point::new(left.x.min(right.x), centre_y - bridge),
                        Size::new(gap, bridge * 2.0),
                        bridge,
                        ink,
                    );
                }
                circle(scene, left, radius * 2.0, ink);
                circle(scene, right, radius * 2.0, ink);
            }
            LoaderVariant::Newton => {
                let ball = base * 0.2;
                let out = ball * 1.1;
                let at = if reduce {
                    0.0
                } else {
                    phase(elapsed, period.mul_f64(1.5), Duration::ZERO)
                };
                for index in 0..NEWTON_BALLS {
                    // Only the end balls move: the left swings out over the
                    // first half, the right over the second.
                    let shift = match index {
                        0 => keyframe(at, &[0.0, 0.28, 0.5, 1.0], &[0.0, -out, 0.0, 0.0]),
                        4 => keyframe(at, &[0.0, 0.5, 0.78, 1.0], &[0.0, 0.0, out, 0.0]),
                        _ => 0.0,
                    };
                    circle(
                        scene,
                        Point::new(
                            origin.x + index as f64 * ball + ball / 2.0 + shift,
                            origin.y + ball / 2.0,
                        ),
                        ball,
                        ink,
                    );
                }
            }
            LoaderVariant::Helix => {
                let dot = base * 0.14;
                let amp = base * 0.32;
                for row in 0..HELIX_ROWS {
                    let y = origin.y
                        + (row as f64 / (HELIX_ROWS - 1) as f64) * (base - dot)
                        + dot / 2.0;
                    let delay = period.mul_f64(row as f64 / HELIX_ROWS as f64);
                    let wave = if reduce {
                        0.5
                    } else {
                        ping_pong(phase(elapsed, period, delay))
                    };
                    let centre_x = origin.x + base / 2.0;
                    // Two strands, exactly out of phase.
                    for (strand, offset) in [(0usize, wave), (1, 1.0 - wave)] {
                        let _ = strand;
                        let x = centre_x + (offset * 2.0 - 1.0) * amp;
                        let scale = 0.5 + offset * 0.5;
                        let alpha = 0.45 + offset * 0.55;
                        circle(
                            scene,
                            Point::new(x, y),
                            dot * scale,
                            scale_alpha(ink, alpha as f32),
                        );
                    }
                }
            }
            LoaderVariant::Percent => {
                let value = self.percent(elapsed, reduce);
                let bar_height = (base * 0.1).max(3.0);
                let gap = base * 0.14;
                let digits: Vec<char> = format!("{value}%").chars().collect();
                let width: f64 = digits
                    .iter()
                    .filter_map(|c| self.glyphs.get(glyph_index(*c)?))
                    .map(|run| run.size().width)
                    .sum();
                let height = self
                    .glyphs
                    .iter()
                    .map(|run| run.size().height)
                    .fold(0.0_f64, f64::max);
                let mut x = origin.x + (size.width - width).max(0.0) / 2.0;
                for character in digits {
                    if let Some(run) = glyph_index(character).and_then(|i| self.glyphs.get(i)) {
                        draw_run(run, Point::new(x, origin.y), &Brush::Solid(ink), scene);
                        x += run.size().width;
                    }
                }
                let track_y = origin.y + height + gap;
                scene.fill_rounded_rect(
                    Point::new(origin.x, track_y),
                    Size::new(size.width, bar_height),
                    bar_height / 2.0,
                    scale_alpha(ink, 0.15),
                );
                let filled = size.width * f64::from(value) / 100.0;
                if filled > 0.0 {
                    scene.fill_rounded_rect(
                        Point::new(origin.x, track_y),
                        Size::new(filled, bar_height),
                        bar_height / 2.0,
                        ink,
                    );
                }
            }
        }
    }
}

/// Where a `character` of the percent variant sits in its shaped glyph list.
fn glyph_index(character: char) -> Option<usize> {
    match character {
        '%' => Some(10),
        digit if digit.is_ascii_digit() => Some(digit as usize - '0' as usize),
        _ => None,
    }
}

/// A filled circle of `diameter` centred on `centre`.
fn circle(scene: &mut dyn PaintScene, centre: Point, diameter: f64, color: Color) {
    if diameter <= 0.0 {
        return;
    }
    scene.fill_rounded_rect(
        Point::new(centre.x - diameter / 2.0, centre.y - diameter / 2.0),
        Size::new(diameter, diameter),
        diameter / 2.0,
        color,
    );
}

/// Emit `run`'s glyphs at `origin` under `brush`.
fn draw_run(run: &TextLayout, origin: Point, brush: &Brush, scene: &mut dyn PaintScene) {
    for mut glyphs in run.to_scene_runs(origin) {
        glyphs.brush = brush.clone();
        scene.draw_glyph_run(glyphs);
    }
}

/// A rotation of `angle` radians about `centre`.
fn rotate_about(angle: f64, centre: Point) -> Affine {
    Affine::translate(centre.to_vec2())
        * Affine::rotate(angle)
        * Affine::translate(-centre.to_vec2())
}

/// The fractional part of `value`, always in `[0, 1)` including for negatives.
fn fract(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    value - value.floor()
}

/// Where a part sits in its own loop at `elapsed`, given a per-part `delay`.
fn phase(elapsed: Duration, period: Duration, delay: Duration) -> f64 {
    if period.is_zero() {
        return 0.0;
    }
    let seconds = elapsed.as_secs_f64() - delay.as_secs_f64();
    fract(seconds / period.as_secs_f64())
}

/// A `0 → 1 → 0` triangle over one loop, eased on both legs — the shape every
/// upstream `[a, b, a]` keyframe set with `EASE_IN_OUT` produces.
fn ping_pong(phase: f64) -> f64 {
    if phase < 0.5 {
        EASE_IN_OUT.transform(phase * 2.0)
    } else {
        1.0 - EASE_IN_OUT.transform((phase - 0.5) * 2.0)
    }
}

/// The alpha the reduced-motion pulse sits at — upstream's
/// `opacity: [1, 0.4, 1]` over [`REDUCED_PERIOD`].
fn reduced_pulse(elapsed: Duration) -> f32 {
    let wave = ping_pong(phase(elapsed, REDUCED_PERIOD, Duration::ZERO)) as f32;
    1.0 - (1.0 - REDUCED_FLOOR) * wave
}

/// Interpolate a keyframe track: `values[i]` at `times[i]`, eased between
/// consecutive pairs — upstream's `transition.times` arrays.
fn keyframe(phase: f64, times: &[f64], values: &[f64]) -> f64 {
    if times.len() < 2 || times.len() != values.len() {
        return 0.0;
    }
    if phase <= times[0] {
        return values[0];
    }
    for window in 1..times.len() {
        if phase <= times[window] {
            let span = times[window] - times[window - 1];
            let local = if span <= 0.0 {
                1.0
            } else {
                (phase - times[window - 1]) / span
            };
            let eased = EASE_IN_OUT.transform(local);
            return values[window - 1] + (values[window] - values[window - 1]) * eased;
        }
    }
    values[values.len() - 1]
}

/// The radius of a regular `sides`-gon at angle `ang`, normalised so its
/// inradius is 1 — upstream's `ngonRadius`.
fn ngon_radius(ang: f64, sides: f64, phase: f64) -> f64 {
    let segment = TAU / sides;
    let a = ang - phase;
    let local = ((a % segment) + segment) % segment - segment / 2.0;
    (PI / sides).cos() / local.cos()
}

/// The five shapes the morph cycles, each sampled at [`MORPH_POINTS`] radii —
/// circle, square, triangle, hexagon, diamond.
fn morph_shapes() -> [[f64; MORPH_POINTS]; 5] {
    let sample = |radius_at: &dyn Fn(f64) -> f64| {
        let mut out = [0.0; MORPH_POINTS];
        for (index, slot) in out.iter_mut().enumerate() {
            let ang = (index as f64 / MORPH_POINTS as f64) * TAU - PI / 2.0;
            *slot = radius_at(ang).min(1.05);
        }
        out
    };
    [
        sample(&|_| 1.0),
        sample(&|a| ngon_radius(a, 4.0, PI / 4.0)),
        sample(&|a| ngon_radius(a, 3.0, 0.0)),
        sample(&|a| ngon_radius(a, 6.0, 0.0)),
        sample(&|a| ngon_radius(a, 4.0, 0.0)),
    ]
}

/// The morph's state at `phase`: the interpolated radii, the rotation and the
/// scale.
///
/// Every shape appears **twice in a row** so it fully forms and holds before the
/// next morph, and the rotation/scale tracks only move across the morphing
/// segments — upstream's `MORPH_SEQ`/`MORPH_ROT`/`MORPH_SCALE`.
fn morph_at(phase: f64) -> ([f64; MORPH_POINTS], f64, f64) {
    let shapes = morph_shapes();
    let sequence = [0usize, 0, 1, 1, 2, 2, 3, 3, 4, 4, 0];
    let rotations = [
        0.0, 0.0, 72.0, 72.0, 144.0, 144.0, 216.0, 216.0, 288.0, 288.0, 360.0,
    ];
    let scales = [1.0, 1.0, 0.88, 0.88, 1.0, 1.0, 0.88, 0.88, 1.0, 1.0, 1.0];

    let segments = sequence.len() - 1;
    let scaled = fract(phase) * segments as f64;
    let index = (scaled.floor() as usize).min(segments - 1);
    let local = EASE_IN_OUT.transform(scaled - index as f64);

    let from = shapes[sequence[index]];
    let to = shapes[sequence[index + 1]];
    let mut radii = [0.0; MORPH_POINTS];
    for (slot, (a, b)) in radii.iter_mut().zip(from.iter().zip(to.iter())) {
        *slot = a + (b - a) * local;
    }
    let rotation = rotations[index] + (rotations[index + 1] - rotations[index]) * local;
    let scale = scales[index] + (scales[index + 1] - scales[index]) * local;
    (radii, rotation.to_radians(), scale)
}

/// A closed polygon from sampled `radii` about `centre` at `radius`.
fn morph_path(radii: &[f64; MORPH_POINTS], centre: Point, radius: f64) -> BezPath {
    let mut path = BezPath::new();
    for (index, r) in radii.iter().enumerate() {
        let ang = (index as f64 / MORPH_POINTS as f64) * TAU - PI / 2.0;
        let at = Point::new(
            centre.x + ang.cos() * radius * r,
            centre.y + ang.sin() * radius * r,
        );
        if index == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    path.close_path();
    path
}

/// A cheap, well-mixed hash of two counters — the scramble's determinism seam
/// (SplitMix64's finalizer, the same construction the text scramble uses).
fn mix(tick: u64, index: u64) -> u64 {
    let mut z = tick
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(index.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust_core::{BuildCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays its loader into.
    const BOX: Size = Size::new(64.0, 64.0);

    /// Counts every drawing operation, so a variant that paints nothing at all
    /// is visible in a test.
    #[derive(Default)]
    struct Recorder {
        ops: usize,
        alphas: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, color: Color) {
            self.ops += 1;
            self.alphas.push(color.components[3]);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, color: Color) {
            self.ops += 1;
            self.alphas.push(color.components[3]);
        }
        fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {
            self.ops += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, brush: &Brush) {
            self.ops += 1;
            if let Brush::Solid(color) = brush {
                self.alphas.push(color.components[3]);
            }
        }
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.ops += 1;
        }
    }

    fn laid_out(view: &LoaderView<()>) -> LoaderWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
        widget
    }

    fn painted(
        widget: &mut LoaderWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, Option<TickClass>) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.frame_class())
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// All seventeen variants the registry entry advertises are constructible,
    /// listed exactly once, and each of them actually draws.
    #[test]
    fn every_registry_variant_is_constructible_and_draws() {
        assert_eq!(LoaderVariant::ALL.len(), 17);
        for (index, variant) in LoaderVariant::ALL.iter().enumerate() {
            assert!(
                !LoaderVariant::ALL[..index].contains(variant),
                "{variant:?} listed twice"
            );
            let mut widget = laid_out(&loader::<()>().variant(*variant));
            assert_eq!(widget.variant(), *variant);
            let (recorder, _) = painted(&mut widget, 120, None);
            assert!(recorder.ops > 0, "{variant:?} painted nothing");
        }
    }

    /// A loader is a decorative loop, so it asks the frame gate for a paceable
    /// cosmetic frame rather than an unpaced transition one — under reduced
    /// motion too, because the pulse is still running.
    #[test]
    fn every_variant_requests_a_paceable_cosmetic_loop() {
        let theme = reduced();
        for variant in LoaderVariant::ALL {
            let mut widget = laid_out(&loader::<()>().variant(variant));
            let (_, class) = painted(&mut widget, 0, None);
            assert_eq!(class, Some(TickClass::CosmeticLoop), "{variant:?}");
            let (_, class) = painted(&mut widget, 0, Some(&theme));
            assert_eq!(class, Some(TickClass::CosmeticLoop), "{variant:?} reduced");
        }
    }

    /// The five terminal-style sets are the ones with frames; nothing else is.
    #[test]
    fn only_the_ascii_variants_carry_frame_sets() {
        let textual = [
            (LoaderVariant::Ascii, 10),
            (LoaderVariant::AsciiLine, 4),
            (LoaderVariant::AsciiBraille, 8),
            (LoaderVariant::AsciiBlocks, 14),
            (LoaderVariant::AsciiBounce, 8),
        ];
        for (variant, frames) in textual {
            assert_eq!(
                variant.ascii_frames().map(<[&str]>::len),
                Some(frames),
                "{variant:?}"
            );
            assert!(variant.is_textual());
        }
        for variant in LoaderVariant::ALL {
            let is_ascii = textual.iter().any(|(named, _)| *named == variant);
            assert_eq!(variant.ascii_frames().is_some(), is_ascii, "{variant:?}");
        }
        // The two other textual variants draw glyphs without being frame sets.
        assert!(LoaderVariant::Scramble.is_textual());
        assert!(LoaderVariant::Percent.is_textual());
        assert!(!LoaderVariant::Spinner.is_textual());
    }

    /// A glyph-cycling variant walks its whole set once per cycle, and runs
    /// slower — not frozen — under reduced motion.
    #[test]
    fn an_ascii_set_cycles_once_per_speed_and_slows_when_calmed() {
        let widget = laid_out(
            &loader::<()>()
                .variant(LoaderVariant::AsciiLine)
                .speed(Duration::from_millis(400)),
        );
        let seen: Vec<usize> = (0..4)
            .map(|step| widget.ascii_index(Duration::from_millis(step * 100), false))
            .collect();
        assert_eq!(seen, vec![0, 1, 2, 3], "one frame per quarter cycle");
        assert_eq!(
            widget.ascii_index(Duration::from_millis(400), false),
            0,
            "and it wraps"
        );
        // Calmed, the set still turns — just further behind at the same instant
        // (upstream slows the cycle rather than stopping it).
        let at = Duration::from_millis(300);
        assert!(
            widget.ascii_index(at, true) < widget.ascii_index(at, false),
            "the calmed cycle is not slower"
        );
        assert!(widget.ascii_index(Duration::from_millis(900), true) > 0);
    }

    /// The scramble resolves left to right, is a pure function of the clock, and
    /// shows its target outright when motion is calmed.
    #[test]
    fn the_scramble_variant_resolves_left_to_right() {
        let widget = laid_out(&loader::<()>().variant(LoaderVariant::Scramble));
        let early = widget.scramble_text(Duration::ZERO, false);
        assert_eq!(early.chars().count(), SCRAMBLE_TARGET.chars().count());
        assert_eq!(
            early,
            widget.scramble_text(Duration::ZERO, false),
            "deterministic"
        );

        // Part way through a cycle the leading letters have resolved.
        let mid = widget.scramble_text(Duration::from_millis(240), false);
        assert!(
            SCRAMBLE_TARGET.starts_with(&mid[..mid.char_indices().nth(2).map_or(0, |(i, _)| i)]),
            "leading letters resolved: {mid}"
        );

        assert_eq!(
            widget.scramble_text(Duration::from_millis(240), true),
            SCRAMBLE_TARGET
        );
    }

    /// The percentage counts a full sweep and wraps rather than sticking.
    #[test]
    fn the_percent_variant_counts_and_wraps() {
        let widget = laid_out(
            &loader::<()>()
                .variant(LoaderVariant::Percent)
                .speed(Duration::from_millis(1000)),
        );
        assert_eq!(widget.percent(Duration::ZERO, false), 0);
        assert_eq!(widget.percent(Duration::from_millis(500), false), 50);
        assert_eq!(widget.percent(Duration::from_millis(1000), false), 0);
        // Calmed, it counts at half the rate.
        assert_eq!(widget.percent(Duration::from_millis(1000), true), 50);
    }

    /// Reduced motion drops every transform and breathes instead: the alphas a
    /// paint emits vary across the pulse while the geometry does not.
    #[test]
    fn reduced_motion_pulses_instead_of_moving() {
        let theme = reduced();
        let mut widget = laid_out(&loader::<()>().variant(LoaderVariant::Dots));
        let (peak, _) = painted(&mut widget, 0, Some(&theme));
        let (trough, _) = painted(&mut widget, 700, Some(&theme));
        let brightest = peak.alphas.iter().copied().fold(0.0_f32, f32::max);
        let dimmest = trough.alphas.iter().copied().fold(0.0_f32, f32::max);
        assert!(
            brightest > dimmest,
            "the pulse never dimmed: {brightest} vs {dimmest}"
        );
        assert!(dimmest >= REDUCED_FLOOR - 1e-3, "dimmed below the floor");
    }

    /// The pulse itself stays inside upstream's `[0.4, 1]` band at every point.
    #[test]
    fn the_reduced_pulse_stays_in_its_band() {
        for step in 0..=100 {
            let at = REDUCED_PERIOD.mul_f64(f64::from(step) / 100.0);
            let alpha = reduced_pulse(at);
            assert!(
                (REDUCED_FLOOR - 1e-6..=1.0 + 1e-6).contains(&alpha),
                "pulse left its band at {at:?}: {alpha}"
            );
        }
        assert!((reduced_pulse(Duration::ZERO) - 1.0).abs() < 1e-6);
        assert!((reduced_pulse(REDUCED_PERIOD.mul_f64(0.5)) - REDUCED_FLOOR).abs() < 1e-6);
    }

    /// The shared triangle wave is anchored at both ends and peaks in the
    /// middle — every `[a, b, a]` keyframe set in this component rides it.
    #[test]
    fn the_ping_pong_wave_is_a_triangle() {
        assert_eq!(ping_pong(0.0), 0.0);
        assert!((ping_pong(0.5) - 1.0).abs() < 1e-9);
        assert!(ping_pong(0.999) < 0.01);
        // Monotone up to the peak, monotone down after it. (Not *symmetric*
        // about the peak: `EASE_IN_OUT`'s own control points are not, which is
        // exactly the asymmetry the ported curve is supposed to keep.)
        let mut previous = 0.0;
        for step in 0..=50 {
            let value = ping_pong(f64::from(step) / 100.0);
            assert!(value >= previous - 1e-9, "climbing leg went backwards");
            previous = value;
        }
        for step in 50..=100 {
            let value = ping_pong(f64::from(step) / 100.0);
            assert!(value <= previous + 1e-9, "falling leg went forwards");
            previous = value;
        }
    }

    /// The loop fold wraps and handles a negative offset (a delayed part before
    /// its own slot opens) rather than producing a negative phase.
    #[test]
    fn the_phase_fold_wraps_including_before_a_delay() {
        let period = Duration::from_millis(1000);
        assert_eq!(phase(Duration::ZERO, period, Duration::ZERO), 0.0);
        assert!((phase(Duration::from_millis(1500), period, Duration::ZERO) - 0.5).abs() < 1e-9);
        let early = phase(
            Duration::from_millis(100),
            period,
            Duration::from_millis(300),
        );
        assert!(
            (0.0..1.0).contains(&early),
            "negative offset folded: {early}"
        );
        // A degenerate period has no phase rather than dividing by zero.
        assert_eq!(
            phase(Duration::from_millis(10), Duration::ZERO, Duration::ZERO),
            0.0
        );
    }

    /// The keyframe track holds its value across a flat segment and moves across
    /// a sloped one — the newton cradle's "only the ends move" shape.
    #[test]
    fn the_keyframe_track_holds_and_moves() {
        let times = [0.0, 0.28, 0.5, 1.0];
        let values = [0.0, -10.0, 0.0, 0.0];
        assert_eq!(keyframe(0.0, &times, &values), 0.0);
        assert!((keyframe(0.28, &times, &values) + 10.0).abs() < 1e-9);
        assert!(keyframe(0.14, &times, &values) < 0.0, "swinging out");
        assert_eq!(keyframe(0.75, &times, &values), 0.0, "at rest on the flat");
        assert_eq!(keyframe(1.0, &times, &values), 0.0);
        // A malformed track is inert rather than a panic.
        assert_eq!(keyframe(0.5, &[0.0], &[1.0]), 0.0);
    }

    /// The morph starts on a circle, holds each shape before moving, and turns
    /// a full revolution over one cycle.
    #[test]
    fn the_morph_holds_each_shape_and_turns_once() {
        let (circle, rotation, scale) = morph_at(0.0);
        assert!(circle.iter().all(|r| (r - 1.0).abs() < 1e-9), "a circle");
        assert_eq!(rotation, 0.0);
        assert_eq!(scale, 1.0);

        // The first segment is a hold: still a circle at its end.
        let (held, held_rotation, _) = morph_at(0.1);
        assert!(
            held.iter().all(|r| (r - 1.0).abs() < 1e-9),
            "still a circle"
        );
        assert_eq!(held_rotation, 0.0);

        // Part way through the second segment it has left the circle behind.
        let (morphing, _, _) = morph_at(0.15);
        assert!(morphing.iter().any(|r| (r - 1.0).abs() > 1e-3), "morphing");

        // And a full cycle is a full turn.
        let (_, full, _) = morph_at(0.999);
        assert!(full > TAU * 0.9, "nearly a full revolution: {full}");
    }

    /// Each variant occupies the box upstream gives it — the wide ones are wide
    /// and the square ones are square.
    #[test]
    fn the_natural_boxes_follow_the_source_metrics() {
        let base = 32.0;
        assert_eq!(
            LoaderVariant::Spinner.natural_size(base),
            Size::new(32.0, 32.0)
        );
        assert_eq!(
            LoaderVariant::Percent.natural_size(base).width,
            base * 1.4,
            "the percent bar is wider than the square"
        );
        let dots = LoaderVariant::Dots.natural_size(base);
        assert!(dots.width > dots.height, "three dots in a row");
        let bars = LoaderVariant::Bars.natural_size(base);
        assert_eq!(bars.height, base);
    }

    /// The explicit ink wins over the theme, and both beat the unthemed
    /// fallback — the catalog's usual resolution ladder.
    #[test]
    fn the_ink_ladder_resolves_explicit_over_theme_over_fallback() {
        let explicit = Color::from_rgb8(0x12, 0x34, 0x56);
        let widget = laid_out(&loader::<()>().color(explicit));
        assert_eq!(widget.ink(None), explicit);

        let plain = laid_out(&loader::<()>());
        assert_eq!(plain.ink(None), FALLBACK_INK);
        let theme = crate::theme();
        assert_eq!(plain.ink(Some(&theme)), theme.scheme().on_surface);
    }

    /// The accessible name is a status label, defaulted and overridable.
    #[test]
    fn the_label_defaults_and_overrides() {
        assert_eq!(laid_out(&loader::<()>()).label(), DEFAULT_LABEL);
        assert_eq!(
            laid_out(&loader::<()>().label("Uploading")).label(),
            "Uploading"
        );
    }
}
