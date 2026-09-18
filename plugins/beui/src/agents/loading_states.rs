//! Ports beUI's `loading-states` agent-interface part.
//!
//! **Source:** `components/agents/loading-states/` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `loading-states`: *"Three thoughtful loading states for AI interfaces:
//! shimmering status text, live agent progress, and cycling reasoning
//! phrases."*
//!
//! The registry entry names one barrel plus three `extraFiles`, and lists three
//! examples. All three arrive here as [`LoadingStatesVariant`] arms of one
//! component rather than three components — the same one-slug-one-component
//! shape [`text_animation`](crate::components::text_animation) takes for its
//! own five-file entry:
//!
//! | upstream example | file | arm |
//! |---|---|---|
//! | `thinking-shimmer` | `thinking-shimmer.tsx` | [`LoadingStatesVariant::Shimmer`] |
//! | `agent-progress` | `agent-progress.tsx` | [`LoadingStatesVariant::Progress`] |
//! | `reasoning-text` | `reasoning-text.tsx` | [`LoadingStatesVariant::Reasoning`] |
//! | — | `message.tsx`'s `MessageTyping` | [`LoadingStatesVariant::Dots`] |
//!
//! [`Dots`](LoadingStatesVariant::Dots) is the one addition: upstream ships its
//! three-dot typing indicator from `message.tsx` rather than from this folder,
//! and the catalog keeps **one** three-dot indicator rather than two identical
//! ones in two modules (see [`super::message`]' export table).
//!
//! # Everything is a pure function of one elapsed time
//!
//! Nothing here holds per-part animation state: each frame is a pure function
//! of the elapsed time since the widget first painted, exactly as a CSS
//! keyframe set is — the same shape [`loader`](crate::components::loader) takes,
//! and what makes every timing here table-testable.
//!
//! # Reduced motion keeps a calm pulse
//!
//! Following [`loader`](crate::components::loader)'s recorded departure from the
//! catalog's usual collapse — *a loading indicator frozen mid-cycle reads as a
//! hung application* — and upstream's own rules, which differ per variant:
//!
//! * [`Dots`](LoadingStatesVariant::Dots) — upstream's reduced rule is a **static**
//!   `opacity: 0.45`, so the dots stop outright and no frames are asked for.
//! * [`Progress`](LoadingStatesVariant::Progress) — upstream keeps
//!   `opacity: [0.35, 0.8, 0.35]` and drops the scale, so the grid keeps its
//!   calm pulse; the timer keeps counting either way.
//! * [`Shimmer`](LoadingStatesVariant::Shimmer) — the sweep stops and the text
//!   paints in its settled ink, matching
//!   [`text_animation`](crate::components::text_animation)'s shimmer.
//! * [`Reasoning`](LoadingStatesVariant::Reasoning) — the phrases keep cycling (a
//!   text swap is not on-screen movement, `loader`'s own precedent) but the
//!   rise and the shimmer are dropped.
//!
//! # Degradations against upstream
//!
//! - **`Reasoning`'s indicator is the dot row, not an ASCII loader.** Upstream
//!   leads the phrase with `<Loader variant="ascii-line" size={14} …/>`. A
//!   caller who wants exactly that composes
//!   [`loader`](crate::components::loader) beside this component; embedding it
//!   here would give this leaf a child pod for one arm out of four.
//! - **`Reasoning` ships one transition, not three.** Upstream's `variant` prop
//!   picks between `cascade` (per-letter), `swap` and `scramble`. Only the swap
//!   is ported; the other two already exist, better, as
//!   [`TextAnimationVariant::Cascade`](crate::components::text_animation::TextAnimationVariant::Cascade)
//!   and
//!   [`Scramble`](crate::components::text_animation::TextAnimationVariant::Scramble),
//!   and a second copy inside a status indicator is not worth its weight.
//! - **`Reasoning` reserves its width by measuring, not by a hidden copy.**
//!   Upstream renders an invisible copy of the longest phrase to stop the row
//!   resizing as phrases cycle; this port measures every phrase at layout time
//!   and reserves the widest — the same result without a second painted run.
//! - **The elapsed timer's cadence is stated, not sampled.** Upstream reads
//!   `performance.now()` on a 100 ms interval; framework-tier code reads no wall
//!   clock, so the timer counts from the frame this widget first painted and
//!   asks for a paced frame every [`PROGRESS_TICK`].

use std::time::Duration;

use frust::authoring::text::{FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx,
    Size, TickClass, View, Widget,
};
use frust::{Curve, FrameTime, Theme};
use kurbo::Point;
use peniko::{Brush, Color, ColorStop, Gradient};

use crate::press::keyframes_at;
use crate::style::{self, scale_alpha};
use crate::text::{ThemeTextType, paint_glyph_run, themed_style};
use crate::tokens::motion::{EASE_IN_OUT, EASE_OUT};
use crate::tokens::{BEUI_LIGHT, mono_family};

/// The dot row's per-dot edge, in logical px (`size-1`).
pub const DOT_SIZE: f64 = 4.0;

/// The gap between dots, in logical px (`gap-1`).
pub const DOT_GAP: f64 = style::SPACING_UNIT;

/// The dot row's own height, in logical px (`h-5`).
pub const DOT_ROW_HEIGHT: f64 = 20.0;

/// One full dot cycle — `duration: 1.05` (`message.tsx`'s `MessageTyping`).
pub const DOT_PERIOD: Duration = Duration::from_millis(1050);

/// Per-dot delay across the row — `delay: index * 0.14`.
pub const DOT_STAGGER: Duration = Duration::from_millis(140);

/// The dot cycle's opacity keyframes — `opacity: [0.28, 0.85, 0.28]`.
const DOT_ALPHA: [f64; 3] = [0.28, 0.85, 0.28];

/// The dot cycle's rise keyframes, in logical px — `y: [0, -2, 0]`.
const DOT_RISE: [f64; 3] = [0.0, -2.0, 0.0];

/// The dots' reduced-motion alpha — upstream's static `{ opacity: 0.45 }`.
const DOT_REDUCED_ALPHA: f32 = 0.45;

/// The activity grid's edge, in logical px (`size-5`).
pub const GRID_EDGE: f64 = 20.0;

/// The gap between grid cells, in logical px (`gap-[2px]`).
pub const GRID_CELL_GAP: f64 = 2.0;

/// The grid cell's corner radius, in logical px (`rounded-[1px]`).
const GRID_CELL_RADIUS: f64 = 1.0;

/// One full grid cycle — `duration: 1.55` (`agent-progress.tsx`).
pub const GRID_PERIOD: Duration = Duration::from_millis(1550);

/// Per-cell delay across the grid — `delay: 0 … 1.12` in steps of `0.14`.
pub const GRID_STAGGER: Duration = Duration::from_millis(140);

/// The grid cycle's opacity keyframes — `opacity: [0.28, 1, 0.28]`.
const GRID_ALPHA: [f64; 3] = [0.28, 1.0, 0.28];

/// The grid cycle's scale keyframes — `scale: [0.72, 1, 0.72]`.
const GRID_SCALE: [f64; 3] = [0.72, 1.0, 0.72];

/// The grid's reduced-motion opacity keyframes — upstream's
/// `{ opacity: [0.35, 0.8, 0.35] }`.
const GRID_REDUCED_ALPHA: [f64; 3] = [0.35, 0.8, 0.35];

/// The gap between the indicator, the label and the timer, in logical px
/// (`gap-3`).
pub const PROGRESS_GAP: f64 = style::SPACING_UNIT * 3.0;

/// How often the elapsed timer is re-read — upstream's 100 ms interval.
pub const PROGRESS_TICK: Duration = Duration::from_millis(100);

/// The gap between the indicator and the text, in logical px (`gap-2`).
pub const INDICATOR_GAP: f64 = style::SPACING_UNIT * 2.0;

/// One full shimmer pass — `duration = 1.8` (`thinking-shimmer.tsx`).
pub const SHIMMER_PERIOD: Duration = Duration::from_millis(1800);

/// One full shimmer pass while reasoning — `shimmerDuration = 2.2`
/// (`reasoning-text.tsx`).
pub const REASONING_SHIMMER_PERIOD: Duration = Duration::from_millis(2200);

/// How long each reasoning phrase stays up — `interval = 1800`.
pub const REASONING_INTERVAL: Duration = Duration::from_millis(1800);

/// Upstream's floor on that interval — `Math.max(600, interval)`.
pub const REASONING_MIN_INTERVAL: Duration = Duration::from_millis(600);

/// How long a reasoning phrase swap takes — `duration: 0.2` on the way in.
pub const REASONING_SWAP_MS: u64 = 200;

/// How far a swapping phrase rises, in logical px (`y: 3 → 0`).
const REASONING_RISE: f64 = 3.0;

/// The shimmer band's leading muted stop (`muted 30%`).
const SHIMMER_LEAD_OFFSET: f32 = 0.3;
/// Where the shimmer's bright stop sits (`foreground 50%`).
const SHIMMER_HIGHLIGHT_OFFSET: f32 = 0.5;
/// The shimmer band's trailing muted stop (`muted 70%`).
const SHIMMER_TRAIL_OFFSET: f32 = 0.7;

/// The status text upstream shows while thinking — `children = "Thinking…"`.
pub const DEFAULT_LABEL: &str = "Thinking…";

/// The verb upstream's progress row leads with — `label = "Churning"`.
pub const DEFAULT_PROGRESS_LABEL: &str = "Churning";

/// The accessible name the dot row publishes — `label = "Responding"`.
pub const DEFAULT_DOTS_LABEL: &str = "Responding";

/// The phrases upstream cycles through — `DEFAULT_PHRASES`, transcribed
/// verbatim.
pub const DEFAULT_PHRASES: [&str; 4] = [
    "Thinking",
    "Reading the context",
    "Connecting the details",
    "Forming a response",
];

/// The ellipsis upstream appends to every reasoning phrase (`${phrase}…`).
const REASONING_SUFFIX: char = '…';

/// Unthemed fallback ink (beUI light `--muted-foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.muted_foreground;
/// Unthemed fallback highlight (beUI light `--foreground`).
const FALLBACK_HIGHLIGHT: Color = BEUI_LIGHT.foreground;

/// Which of the four agent loading states a [`LoadingStatesView`] paints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadingStatesVariant {
    /// The three-dot typing indicator (`MessageTyping`): each dot fades and
    /// rises on its own [`DOT_STAGGER`] offset.
    Dots,
    /// `thinking-shimmer`: a bright band sweeps across the status text forever.
    #[default]
    Shimmer,
    /// `agent-progress`: a 3x3 activity glyph, an action verb, and a live
    /// tabular timer.
    Progress,
    /// `reasoning-text`: shimmering copy that swaps phrase every
    /// [`REASONING_INTERVAL`].
    Reasoning,
}

impl LoadingStatesVariant {
    /// Every variant, in the order the [module docs](self)' table lists them.
    pub const ALL: [LoadingStatesVariant; 4] = [
        LoadingStatesVariant::Dots,
        LoadingStatesVariant::Shimmer,
        LoadingStatesVariant::Progress,
        LoadingStatesVariant::Reasoning,
    ];

    /// Whether this variant paints text at all — [`Dots`](Self::Dots) is
    /// glyph-free (its label is published to accessibility only, upstream's
    /// `sr-only` span).
    pub const fn has_text(self) -> bool {
        !matches!(self, LoadingStatesVariant::Dots)
    }

    /// Whether this variant leads with an indicator glyph.
    pub const fn has_indicator(self) -> bool {
        matches!(
            self,
            LoadingStatesVariant::Dots
                | LoadingStatesVariant::Progress
                | LoadingStatesVariant::Reasoning
        )
    }

    /// This variant's default status text.
    pub const fn default_label(self) -> &'static str {
        match self {
            LoadingStatesVariant::Dots => DEFAULT_DOTS_LABEL,
            LoadingStatesVariant::Progress => DEFAULT_PROGRESS_LABEL,
            _ => DEFAULT_LABEL,
        }
    }
}

/// `elapsed` folded into `[0, 1)` of a `period`-long loop; a zero period has no
/// cycle and reports `0.0`.
fn cycle(elapsed: Duration, period: Duration) -> f64 {
    if period.is_zero() {
        return 0.0;
    }
    let turns = elapsed.as_secs_f64() / period.as_secs_f64();
    turns - turns.floor()
}

/// Where one staggered part of a repeating cycle is at `elapsed`: its own
/// delay subtracted, then folded into the cycle. A part whose delay has not
/// elapsed yet sits at the cycle's start rather than running backwards.
fn staggered_cycle(elapsed: Duration, period: Duration, delay: Duration) -> f64 {
    cycle(elapsed.saturating_sub(delay), period)
}

/// Sample an evenly-spaced keyframe array at `t`, easing the fraction *within
/// each segment* rather than across the whole timeline — Motion's own
/// behaviour for an array target with an `ease`, and the reason a symmetric
/// `[a, b, a]` array stays symmetric under an eased read.
///
/// [`keyframes_at`] is the linear form this is built on.
fn eased_keyframes(frames: &[f64], t: f64, curve: Curve) -> f64 {
    let len = frames.len();
    if len < 2 {
        return keyframes_at(frames, t);
    }
    let spans = (len - 1) as f64;
    let scaled = t.clamp(0.0, 1.0) * spans;
    let index = (scaled.floor() as usize).min(len - 2);
    let local = curve.transform(scaled - index as f64);
    frames[index] + (frames[index + 1] - frames[index]) * local
}

/// Format `seconds` the way upstream's `formatElapsed` does: `"12.3s"` below a
/// minute, `"1m 02.5s"` above it — one decimal place either way, and never
/// negative.
pub fn format_elapsed(seconds: f64) -> String {
    let safe = seconds.max(0.0);
    let minutes = (safe / 60.0).floor();
    let rest = safe - minutes * 60.0;
    if minutes > 0.0 {
        format!("{minutes:.0}m {rest:04.1}s")
    } else {
        format!("{rest:.1}s")
    }
}

/// A declarative beUI agent loading state. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::agents::loading_states::{LoadingStatesVariant, loading_states};
///
/// let thinking = loading_states().variant(LoadingStatesVariant::Reasoning);
/// let working = loading_states()
///     .variant(LoadingStatesVariant::Progress)
///     .label("Churning")
///     .elapsed_seconds(12.4);
/// ```
pub struct LoadingStatesView {
    variant: LoadingStatesVariant,
    label: Option<String>,
    phrases: Vec<String>,
    interval: Option<Duration>,
    duration: Option<Duration>,
    elapsed_seconds: Option<f64>,
    initial_seconds: f64,
    running: bool,
}

/// Create a loading state playing [`LoadingStatesVariant::Shimmer`] over
/// [`DEFAULT_LABEL`].
pub fn loading_states() -> LoadingStatesView {
    LoadingStatesView {
        variant: LoadingStatesVariant::default(),
        label: None,
        phrases: Vec::new(),
        interval: None,
        duration: None,
        elapsed_seconds: None,
        initial_seconds: 0.0,
        running: true,
    }
}

impl LoadingStatesView {
    /// Paint `variant` instead of [`Shimmer`](LoadingStatesVariant::Shimmer).
    pub fn variant(mut self, variant: LoadingStatesVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Replace the status text (the shimmer's copy, the progress row's verb,
    /// the dot row's accessible name).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Cycle `phrases` instead of [`DEFAULT_PHRASES`]
    /// ([`Reasoning`](LoadingStatesVariant::Reasoning) only). An empty list
    /// falls back to the defaults, exactly as upstream's `safePhrases` guard
    /// does.
    pub fn phrases<S: Into<String>>(mut self, phrases: impl IntoIterator<Item = S>) -> Self {
        self.phrases = phrases.into_iter().map(Into::into).collect();
        self
    }

    /// How long each reasoning phrase stays up (default
    /// [`REASONING_INTERVAL`]), floored at [`REASONING_MIN_INTERVAL`].
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    /// Override the shimmer's period (default [`SHIMMER_PERIOD`], or
    /// [`REASONING_SHIMMER_PERIOD`] while reasoning).
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Drive the progress timer from the app instead of from this widget's own
    /// frame clock (`elapsedSeconds`).
    pub fn elapsed_seconds(mut self, seconds: f64) -> Self {
        self.elapsed_seconds = Some(seconds);
        self
    }

    /// Start the internal timer at `seconds` rather than zero
    /// (`initialSeconds`).
    pub fn initial_seconds(mut self, seconds: f64) -> Self {
        self.initial_seconds = seconds;
        self
    }

    /// Whether the internal timer advances (default `true`, `running`). Ignored
    /// when [`elapsed_seconds`](Self::elapsed_seconds) is supplied.
    pub fn running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    /// The phrase list this view cycles, defaults folded in.
    fn resolved_phrases(&self) -> Vec<String> {
        if self.phrases.is_empty() {
            DEFAULT_PHRASES.iter().map(|s| (*s).to_owned()).collect()
        } else {
            self.phrases.clone()
        }
    }

    /// The status text this view shows, defaults folded in.
    fn resolved_label(&self) -> String {
        self.label
            .clone()
            .unwrap_or_else(|| self.variant.default_label().to_owned())
    }
}

/// The retained widget for a [`LoadingStatesView`].
pub struct LoadingStatesWidget {
    variant: LoadingStatesVariant,
    label: String,
    phrases: Vec<String>,
    interval: Option<Duration>,
    duration: Option<Duration>,
    elapsed_seconds: Option<f64>,
    initial_seconds: f64,
    running: bool,
    /// The string currently shaped into [`Self::run`] — the label, or the
    /// reasoning phrase on screen.
    display: String,
    /// The shaped status run, plus what it was shaped from.
    run: Option<TextLayout>,
    shaped: Option<(String, TextStyle)>,
    /// The shaped elapsed timer, plus what it was shaped from
    /// ([`Progress`](LoadingStatesVariant::Progress) only).
    timer: Option<TextLayout>,
    timer_shaped: Option<(String, TextStyle)>,
    /// The timer text `paint` last computed and `layout` owes a shape for.
    timer_text: String,
    /// The width the widest phrase reserves, so cycling never resizes the row.
    reserved_text_width: f64,
    /// The frame this widget first painted on — every cycle is measured from it.
    started: Option<FrameTime>,
    /// Where the status run sits inside the row, resolved by layout.
    text_at: Point,
    /// Where the timer sits inside the row, resolved by layout.
    timer_at: Point,
    /// Where the indicator sits inside the row, resolved by layout.
    indicator_at: Point,
    /// The row's own box, resolved by layout.
    row: Size,
}

impl LoadingStatesWidget {
    /// Which state this widget paints.
    pub fn variant(&self) -> LoadingStatesVariant {
        self.variant
    }

    /// The string currently on screen: the reasoning phrase, or the label.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// How long each reasoning phrase stays up, upstream's floor applied.
    pub fn phrase_interval(&self) -> Duration {
        self.interval
            .unwrap_or(REASONING_INTERVAL)
            .max(REASONING_MIN_INTERVAL)
    }

    /// Which phrase is up at `elapsed` — `Math.floor(elapsed / interval) %
    /// phrases.len()`. A single-phrase list never cycles (upstream's
    /// `safePhrases.length < 2` guard).
    pub fn phrase_index(&self, elapsed: Duration) -> usize {
        if self.phrases.len() < 2 {
            return 0;
        }
        let interval = self.phrase_interval().as_secs_f64();
        let turns = (elapsed.as_secs_f64() / interval).floor() as usize;
        turns % self.phrases.len()
    }

    /// The phrase up at `elapsed`, ellipsis appended.
    fn phrase_at(&self, elapsed: Duration) -> String {
        let index = self.phrase_index(elapsed);
        let mut phrase = self
            .phrases
            .get(index)
            .cloned()
            .unwrap_or_else(|| DEFAULT_LABEL.to_owned());
        phrase.push(REASONING_SUFFIX);
        phrase
    }

    /// The seconds the progress timer shows at `elapsed`: the controlled
    /// override, else the internal count from this widget's first paint (frozen
    /// at [`initial_seconds`](LoadingStatesView::initial_seconds) while not
    /// running).
    pub fn elapsed_at(&self, elapsed: Duration) -> f64 {
        match self.elapsed_seconds {
            Some(seconds) => seconds,
            None if self.running => self.initial_seconds + elapsed.as_secs_f64(),
            None => self.initial_seconds,
        }
    }

    /// The shimmer period this variant sweeps over.
    fn shimmer_period(&self) -> Duration {
        self.duration.unwrap_or(match self.variant {
            LoadingStatesVariant::Reasoning => REASONING_SHIMMER_PERIOD,
            _ => SHIMMER_PERIOD,
        })
    }

    /// The status run's style: `text-sm font-medium` in the theme's
    /// `body_medium` family, shaped in `ink`.
    fn text_style(&self, theme: Option<&Theme>, ink: Color) -> TextStyle {
        let style = TextStyle {
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(style::TEXT_SM as f32, ink)
        };
        themed_style(style, ThemeTextType::BodyMedium, theme)
    }

    /// The timer's style: `font-mono tabular-nums`, one alpha step dimmer than
    /// the label (`text-muted-foreground/70`).
    fn timer_style(&self, ink: Color) -> TextStyle {
        TextStyle {
            // Explicit: `TypeScale` has no monospace role to take this from.
            family: mono_family(),
            ..TextStyle::new(style::TEXT_SM as f32, scale_alpha(ink, 0.7))
        }
    }

    /// The settled ink: the theme's dimmed on-surface role, else the beUI-light
    /// fallback.
    fn ink(&self, theme: Option<&Theme>) -> Color {
        theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface_variant)
    }

    /// The shimmer's bright stop.
    fn highlight(&self, theme: Option<&Theme>) -> Color {
        theme.map_or(FALLBACK_HIGHLIGHT, |t| t.scheme().on_surface)
    }

    /// The indicator's own box for this variant.
    fn indicator_size(&self) -> Size {
        match self.variant {
            LoadingStatesVariant::Dots | LoadingStatesVariant::Reasoning => Size::new(
                DOT_SIZE * 3.0 + DOT_GAP * 2.0,
                if self.variant == LoadingStatesVariant::Dots {
                    DOT_ROW_HEIGHT
                } else {
                    DOT_SIZE
                },
            ),
            LoadingStatesVariant::Progress => Size::new(GRID_EDGE, GRID_EDGE),
            LoadingStatesVariant::Shimmer => Size::ZERO,
        }
    }

    /// The gap between the indicator and the text for this variant.
    fn indicator_gap(&self) -> f64 {
        match self.variant {
            LoadingStatesVariant::Progress => PROGRESS_GAP,
            _ => INDICATOR_GAP,
        }
    }

    /// Paint the three-dot indicator at `at`, in `ink`.
    fn paint_dots(
        &self,
        scene: &mut dyn PaintScene,
        at: Point,
        ink: Color,
        elapsed: Duration,
        reduce: bool,
    ) {
        let mid_y = at.y + self.indicator_size().height / 2.0;
        for index in 0..3 {
            let x = at.x + index as f64 * (DOT_SIZE + DOT_GAP);
            let (alpha, rise) = if reduce {
                (DOT_REDUCED_ALPHA as f64, 0.0)
            } else {
                let t = staggered_cycle(elapsed, DOT_PERIOD, DOT_STAGGER * index as u32);
                (
                    eased_keyframes(&DOT_ALPHA, t, EASE_OUT),
                    eased_keyframes(&DOT_RISE, t, EASE_OUT),
                )
            };
            scene.fill_rounded_rect(
                Point::new(x, mid_y - DOT_SIZE / 2.0 + rise),
                Size::new(DOT_SIZE, DOT_SIZE),
                DOT_SIZE / 2.0,
                scale_alpha(ink, alpha as f32),
            );
        }
    }

    /// Paint the 3x3 activity grid at `at`, in `ink`.
    fn paint_grid(
        &self,
        scene: &mut dyn PaintScene,
        at: Point,
        ink: Color,
        elapsed: Duration,
        reduce: bool,
    ) {
        let cell = (GRID_EDGE - GRID_CELL_GAP * 2.0) / 3.0;
        for row in 0..3 {
            for column in 0..3 {
                let index = row * 3 + column;
                let t = staggered_cycle(elapsed, GRID_PERIOD, GRID_STAGGER * index as u32);
                let (alpha, scale) = if reduce {
                    (eased_keyframes(&GRID_REDUCED_ALPHA, t, EASE_IN_OUT), 1.0)
                } else {
                    (
                        eased_keyframes(&GRID_ALPHA, t, EASE_IN_OUT),
                        eased_keyframes(&GRID_SCALE, t, EASE_IN_OUT),
                    )
                };
                // The scale is applied to the cell's own box about its centre,
                // which is what `scale` on a grid child means.
                let edge = cell * scale;
                let inset = (cell - edge) / 2.0;
                scene.fill_rounded_rect(
                    Point::new(
                        at.x + column as f64 * (cell + GRID_CELL_GAP) + inset,
                        at.y + row as f64 * (cell + GRID_CELL_GAP) + inset,
                    ),
                    Size::new(edge, edge),
                    GRID_CELL_RADIUS,
                    scale_alpha(ink, alpha as f32),
                );
            }
        }
    }
}

impl<State: 'static> View<State> for LoadingStatesView {
    type Element = LoadingStatesWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> LoadingStatesWidget {
        let label = self.resolved_label();
        LoadingStatesWidget {
            variant: self.variant,
            display: label.clone(),
            label,
            phrases: self.resolved_phrases(),
            interval: self.interval,
            duration: self.duration,
            elapsed_seconds: self.elapsed_seconds,
            initial_seconds: self.initial_seconds,
            running: self.running,
            run: None,
            shaped: None,
            timer: None,
            timer_shaped: None,
            timer_text: format_elapsed(self.elapsed_seconds.unwrap_or(self.initial_seconds)),
            reserved_text_width: 0.0,
            started: None,
            text_at: Point::ORIGIN,
            timer_at: Point::ORIGIN,
            indicator_at: Point::ORIGIN,
            row: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut LoadingStatesWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.variant != self.variant {
            element.variant = self.variant;
            // Restart: each variant's cycle is measured from its own first
            // frame, and the row's shape changes with the indicator.
            element.started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let label = self.resolved_label();
        if element.label != label {
            element.label = label.clone();
            if element.variant != LoadingStatesVariant::Reasoning {
                element.display = label;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let phrases = self.resolved_phrases();
        if element.phrases != phrases {
            element.phrases = phrases;
            element.started = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.interval != self.interval {
            element.interval = self.interval;
            flags |= ChangeFlags::PAINT;
        }
        if element.duration != self.duration {
            element.duration = self.duration;
            flags |= ChangeFlags::PAINT;
        }
        if element.elapsed_seconds != self.elapsed_seconds {
            element.elapsed_seconds = self.elapsed_seconds;
            // The timer's own width can change with its value.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.initial_seconds != self.initial_seconds {
            element.initial_seconds = self.initial_seconds;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.running != self.running {
            element.running = self.running;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, _element: &mut LoadingStatesWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for LoadingStatesWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let ink = self.ink(theme);
        let style = self.text_style(theme, ink);
        let timer_style = self.timer_style(ink);

        let mut text_size = Size::ZERO;
        if self.variant.has_text() {
            let wanted = self.display.clone();
            if self.shaped.as_ref() != Some(&(wanted.clone(), style.clone())) {
                let text_ctx = ctx.text_context::<TextContext>();
                self.run = Some(text_ctx.layout(&wanted, &style, None));
                self.shaped = Some((wanted, style.clone()));
            }
            text_size = self.run.as_ref().map_or(Size::ZERO, TextLayout::size);
        }

        // Reserve the widest phrase's width so cycling never resizes the row —
        // upstream renders an invisible copy of the longest phrase for the same
        // reason.
        self.reserved_text_width = text_size.width;
        if self.variant == LoadingStatesVariant::Reasoning {
            let phrases: Vec<String> = self
                .phrases
                .iter()
                .map(|phrase| format!("{phrase}{REASONING_SUFFIX}"))
                .collect();
            let text_ctx = ctx.text_context::<TextContext>();
            for phrase in &phrases {
                let measured = text_ctx.layout(phrase, &style, None).size();
                self.reserved_text_width = self.reserved_text_width.max(measured.width);
            }
        }

        let mut timer_size = Size::ZERO;
        if self.variant == LoadingStatesVariant::Progress {
            let wanted = self.timer_text.clone();
            if self.timer_shaped.as_ref() != Some(&(wanted.clone(), timer_style.clone())) {
                let text_ctx = ctx.text_context::<TextContext>();
                self.timer = Some(text_ctx.layout(&wanted, &timer_style, None));
                self.timer_shaped = Some((wanted, timer_style));
            }
            timer_size = self.timer.as_ref().map_or(Size::ZERO, TextLayout::size);
        }

        let indicator = self.indicator_size();
        let gap = if self.variant.has_indicator() && self.variant.has_text() {
            self.indicator_gap()
        } else {
            0.0
        };
        let mut width = indicator.width + gap + self.reserved_text_width;
        if timer_size.width > 0.0 {
            width += PROGRESS_GAP + timer_size.width;
        }
        let height = indicator
            .height
            .max(text_size.height)
            .max(timer_size.height);

        // `items-center`: everything sits on the row's own centre line.
        let centre = |h: f64| (height - h) / 2.0;
        self.indicator_at = Point::new(0.0, centre(indicator.height));
        self.text_at = Point::new(indicator.width + gap, centre(text_size.height));
        self.timer_at = Point::new(
            self.text_at.x + self.reserved_text_width + PROGRESS_GAP,
            centre(timer_size.height),
        );
        self.row = Size::new(width, height);
        bc.constrain(self.row)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink = self.ink(theme);
        let highlight = self.highlight(theme);

        let now = ctx.frame_time();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let origin = ctx.origin();

        // 1. The indicator.
        let indicator_at = Point::new(
            origin.x + self.indicator_at.x,
            origin.y + self.indicator_at.y,
        );
        match self.variant {
            LoadingStatesVariant::Dots | LoadingStatesVariant::Reasoning => {
                self.paint_dots(scene, indicator_at, ink, elapsed, reduce);
            }
            LoadingStatesVariant::Progress => {
                self.paint_grid(scene, indicator_at, ink, elapsed, reduce);
            }
            LoadingStatesVariant::Shimmer => {}
        }

        // 2. The phrase currently up — a swap re-shapes, so it is requested
        // rather than performed here (shaping only happens in `layout`).
        if self.variant == LoadingStatesVariant::Reasoning {
            let wanted = self.phrase_at(elapsed);
            if wanted != self.display {
                self.display = wanted;
                ctx.request_layout();
            }
        }

        // 3. The status run, shimmered unless reduced.
        let text_at = Point::new(origin.x + self.text_at.x, origin.y + self.text_at.y);
        if self.variant.has_text() {
            let size = self.run.as_ref().map_or(Size::ZERO, TextLayout::size);
            let brush = if reduce || self.variant == LoadingStatesVariant::Progress {
                Brush::Solid(ink)
            } else {
                Brush::Gradient(shimmer_gradient(
                    text_at,
                    size,
                    cycle(elapsed, self.shimmer_period()),
                    ink,
                    highlight,
                ))
            };
            // A freshly-swapped phrase rises into place (`y: 3 → 0`), which is
            // a paint-time offset rather than a relayout.
            let rise = if self.variant == LoadingStatesVariant::Reasoning && !reduce {
                let interval = self.phrase_interval().as_secs_f64();
                let into = elapsed.as_secs_f64() % interval.max(f64::EPSILON);
                let swap = (into / (REASONING_SWAP_MS as f64 / 1000.0)).clamp(0.0, 1.0);
                REASONING_RISE * (1.0 - EASE_OUT.transform(swap))
            } else {
                0.0
            };
            paint_glyph_run(
                self.run.as_ref(),
                Point::new(text_at.x, text_at.y + rise),
                &brush,
                scene,
            );
        }

        // 4. The elapsed timer.
        if self.variant == LoadingStatesVariant::Progress {
            let wanted = format_elapsed(self.elapsed_at(elapsed));
            if wanted != self.timer_text {
                self.timer_text = wanted;
                ctx.request_layout();
            }
            paint_glyph_run(
                self.timer.as_ref(),
                Point::new(origin.x + self.timer_at.x, origin.y + self.timer_at.y),
                &Brush::Solid(scale_alpha(ink, 0.7)),
                scene,
            );
        }

        // 5. What the next frame is owed. Every loop here is decorative and
        // perpetual, never a transition — except the dot row under reduced
        // motion, which upstream freezes outright.
        let frozen = reduce && self.variant == LoadingStatesVariant::Dots;
        if !frozen {
            match self.variant {
                // The timer only changes every 100ms, far slower than the frame
                // gate's cosmetic cap, so its cadence is stated.
                LoadingStatesVariant::Progress if self.elapsed_seconds.is_none() => {
                    ctx.request_frame_paced_at(PROGRESS_TICK);
                }
                _ => ctx.request_frame_class(TickClass::CosmeticLoop),
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `role="status"` + `aria-live="polite"` upstream; the label is the
        // resolved phrase, never the shimmer's sampled paint.
        let value = if self.variant == LoadingStatesVariant::Reasoning {
            self.display.clone()
        } else {
            self.label.clone()
        };
        ctx.push_node(Role::Status, |node| {
            node.set_label(value.as_str());
        });
    }
}

/// The shimmer's band at `phase`: a `base` → `highlight` → `base` gradient two
/// run-widths wide, travelling left to right across the run exactly once per
/// cycle.
///
/// Built per frame because a gradient's geometry is resolved where it is
/// encoded, and because a `GlyphRun`'s brush is transformed with the run — the
/// endpoints are therefore stated relative to the run's own origin.
fn shimmer_gradient(
    origin: Point,
    size: Size,
    phase: f64,
    base: Color,
    highlight: Color,
) -> Gradient {
    let band = (size.width * 2.0).max(1.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use std::any::Any;

    /// The box every test lays its row into.
    const BOX: Size = Size::new(320.0, 40.0);

    /// Records every rounded-rect fill and every glyph brush.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        brushes: Vec<Brush>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.brushes.push(run.brush);
        }
    }

    fn laid_out(view: &LoadingStatesView) -> LoadingStatesWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut LoadingStatesWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
    }

    /// Paint at `ms`, returning what was drawn plus the frame the widget asked
    /// for.
    fn painted(
        widget: &mut LoadingStatesWidget,
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

    /// Every variant is constructible, paints something, and keeps asking for
    /// frames — a loading indicator that stops is a hung application.
    #[test]
    fn every_variant_paints_and_keeps_running() {
        assert_eq!(LoadingStatesVariant::ALL.len(), 4);
        for variant in LoadingStatesVariant::ALL {
            let mut widget = laid_out(&loading_states().variant(variant));
            let (rec, needs_frame, ..) = painted(&mut widget, 0, None);
            assert_eq!(widget.variant(), variant);
            assert!(needs_frame, "{variant:?} keeps running");
            assert!(
                !rec.rounded.is_empty() || !rec.brushes.is_empty(),
                "{variant:?} paints something"
            );
            assert_eq!(
                rec.brushes.is_empty(),
                !variant.has_text(),
                "{variant:?} text presence"
            );
        }
        // No arm is listed twice.
        for (index, variant) in LoadingStatesVariant::ALL.iter().enumerate() {
            assert!(!LoadingStatesVariant::ALL[..index].contains(variant));
        }
    }

    /// The dot row is three dots whose alpha and rise are staggered
    /// [`DOT_STAGGER`] apart — never all three at the same point of the cycle.
    #[test]
    fn the_dots_run_on_staggered_offsets() {
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Dots));
        // The first paint latches the cycle clock, so the offsets are read on
        // the second: 350ms in, the three delays (0/140/280) put the dots at
        // three distinct points of a 1050ms cycle.
        painted(&mut widget, 0, None);
        let (rec, ..) = painted(&mut widget, 350, None);
        assert_eq!(rec.rounded.len(), 3, "three dots");
        let alphas: Vec<f32> = rec
            .rounded
            .iter()
            .map(|(_, _, c)| c.components[3])
            .collect();
        assert!(
            alphas[0] != alphas[1] && alphas[1] != alphas[2],
            "staggered: {alphas:?}"
        );
        let ys: Vec<f64> = rec.rounded.iter().map(|(p, _, _)| p.y).collect();
        assert!(
            ys[0] != ys[1] || ys[1] != ys[2],
            "the rise is staggered too"
        );
    }

    /// Upstream's reduced rule for the dots is a *static* `opacity: 0.45`, so
    /// the row stops outright and asks for nothing further.
    #[test]
    fn reduce_motion_freezes_the_dots_at_a_flat_alpha() {
        let theme = reduced();
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Dots));
        painted(&mut widget, 0, Some(&theme));
        let (rec, needs_frame, ..) = painted(&mut widget, 350, Some(&theme));
        assert!(!needs_frame, "a frozen indicator owes no frames");
        assert_eq!(rec.rounded.len(), 3);
        for (_, _, color) in &rec.rounded {
            assert!((color.components[3] - DOT_REDUCED_ALPHA).abs() < 1e-6);
        }
        let ys: Vec<f64> = rec.rounded.iter().map(|(p, _, _)| p.y).collect();
        assert!(
            ys.iter().all(|y| *y == ys[0]),
            "no rise under reduced motion"
        );
    }

    /// The grid is nine cells, each on its own cycle offset; under reduced
    /// motion the scale is dropped and only the opacity pulses.
    #[test]
    fn the_grid_pulses_nine_cells_and_drops_its_scale_when_reduced() {
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Progress));
        painted(&mut widget, 0, None);
        let (rec, needs_frame, _, paced, _) = painted(&mut widget, 400, None);
        assert!(needs_frame);
        assert_eq!(paced, Some(PROGRESS_TICK), "the timer states its cadence");
        assert_eq!(rec.rounded.len(), 9, "a 3x3 grid");
        let edges: Vec<f64> = rec.rounded.iter().map(|(_, s, _)| s.width).collect();
        assert!(
            edges.iter().any(|e| (*e - edges[0]).abs() > 1e-6),
            "the cells scale independently: {edges:?}"
        );

        let theme = reduced();
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Progress));
        painted(&mut widget, 0, Some(&theme));
        let (rec, needs_frame, ..) = painted(&mut widget, 400, Some(&theme));
        assert!(needs_frame, "the calm pulse keeps running");
        let edges: Vec<f64> = rec.rounded.iter().map(|(_, s, _)| s.width).collect();
        assert!(
            edges.iter().all(|e| (*e - edges[0]).abs() < 1e-9),
            "reduced motion drops the scale: {edges:?}"
        );
    }

    /// `formatElapsed`, transcribed: one decimal below a minute, `Nm SS.Ss`
    /// above it, never negative.
    #[test]
    fn the_elapsed_timer_is_formatted_upstreams_way() {
        assert_eq!(format_elapsed(0.0), "0.0s");
        assert_eq!(format_elapsed(12.44), "12.4s");
        assert_eq!(format_elapsed(59.9), "59.9s");
        assert_eq!(format_elapsed(60.0), "1m 00.0s");
        assert_eq!(format_elapsed(125.5), "2m 05.5s");
        assert_eq!(format_elapsed(-5.0), "0.0s", "never negative");
    }

    /// A controlled `elapsedSeconds` wins over the internal timer; an
    /// uncontrolled one counts from the first painted frame, and `running=false`
    /// holds it at its starting value.
    #[test]
    fn the_timer_honours_control_and_the_running_flag() {
        let controlled = laid_out(
            &loading_states()
                .variant(LoadingStatesVariant::Progress)
                .elapsed_seconds(41.0),
        );
        assert_eq!(controlled.elapsed_at(Duration::from_secs(9)), 41.0);

        let internal = laid_out(
            &loading_states()
                .variant(LoadingStatesVariant::Progress)
                .initial_seconds(2.0),
        );
        assert_eq!(internal.elapsed_at(Duration::from_secs(3)), 5.0);

        let held = laid_out(
            &loading_states()
                .variant(LoadingStatesVariant::Progress)
                .initial_seconds(2.0)
                .running(false),
        );
        assert_eq!(held.elapsed_at(Duration::from_secs(3)), 2.0);
    }

    /// The reasoning row cycles its phrases on the interval, wrapping round,
    /// and a single-phrase list never cycles at all.
    #[test]
    fn the_reasoning_phrases_cycle_on_the_interval() {
        let widget = laid_out(&loading_states().variant(LoadingStatesVariant::Reasoning));
        assert_eq!(widget.phrase_interval(), REASONING_INTERVAL);
        assert_eq!(widget.phrase_index(Duration::ZERO), 0);
        assert_eq!(widget.phrase_index(Duration::from_millis(1799)), 0);
        assert_eq!(widget.phrase_index(Duration::from_millis(1800)), 1);
        assert_eq!(widget.phrase_index(Duration::from_millis(3600)), 2);
        // Four default phrases, so the fifth interval wraps to the first.
        assert_eq!(widget.phrase_index(Duration::from_millis(7200)), 0);

        let single = laid_out(
            &loading_states()
                .variant(LoadingStatesVariant::Reasoning)
                .phrases(["Only one"]),
        );
        assert_eq!(single.phrase_index(Duration::from_secs(60)), 0);
    }

    /// Upstream floors the interval at 600 ms (`Math.max(600, interval)`).
    #[test]
    fn the_phrase_interval_is_floored() {
        let widget = laid_out(
            &loading_states()
                .variant(LoadingStatesVariant::Reasoning)
                .interval(Duration::from_millis(10)),
        );
        assert_eq!(widget.phrase_interval(), REASONING_MIN_INTERVAL);
    }

    /// An empty phrase list falls back to upstream's own four
    /// (`safePhrases`).
    #[test]
    fn an_empty_phrase_list_falls_back_to_the_defaults() {
        let widget = laid_out(
            &loading_states()
                .variant(LoadingStatesVariant::Reasoning)
                .phrases(Vec::<String>::new()),
        );
        assert_eq!(widget.phrase_index(Duration::from_millis(1800)), 1);
        assert_eq!(widget.display(), "Thinking…");
    }

    /// A phrase swap is a different shaped run, so the paint that notices it
    /// asks for a relayout rather than shaping mid-paint.
    #[test]
    fn a_phrase_swap_requests_a_relayout() {
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Reasoning));
        let (_, _, _, _, needs_layout) = painted(&mut widget, 0, None);
        assert!(!needs_layout, "the first phrase is already shaped");
        assert_eq!(widget.display(), "Thinking…");

        let (_, _, _, _, needs_layout) = painted(&mut widget, 1_900, None);
        assert!(needs_layout, "the swap owes a re-shape");
        assert_eq!(widget.display(), "Reading the context…");
    }

    /// The reasoning row reserves the widest phrase's width, so cycling never
    /// resizes it — upstream's invisible longest-phrase copy, measured instead.
    #[test]
    fn the_reasoning_row_reserves_the_widest_phrase() {
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Reasoning));
        let first = widget.row;
        painted(&mut widget, 0, None);
        painted(&mut widget, 1_900, None);
        relayout(&mut widget);
        assert_eq!(widget.display(), "Reading the context…");
        assert_eq!(
            widget.row, first,
            "the row does not resize as phrases cycle"
        );
    }

    /// The shimmer paints its run under a gradient brush and stops sweeping
    /// under reduced motion.
    #[test]
    fn the_shimmer_sweeps_a_gradient_and_settles_when_reduced() {
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Shimmer));
        let (rec, needs_frame, class, ..) = painted(&mut widget, 400, None);
        assert!(needs_frame);
        assert_eq!(class, Some(TickClass::CosmeticLoop), "a decorative loop");
        assert!(
            rec.brushes.iter().any(|b| matches!(b, Brush::Gradient(_))),
            "the sweep is a gradient"
        );

        let theme = reduced();
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Shimmer));
        let (rec, ..) = painted(&mut widget, 400, Some(&theme));
        assert!(
            rec.brushes.iter().all(|b| matches!(b, Brush::Solid(_))),
            "reduced motion paints settled ink"
        );
    }

    /// The progress row's own text is never shimmered — upstream's verb is
    /// plain `font-medium`, and only its grid animates.
    #[test]
    fn the_progress_row_paints_plain_ink() {
        let mut widget = laid_out(&loading_states().variant(LoadingStatesVariant::Progress));
        let (rec, ..) = painted(&mut widget, 400, None);
        assert!(
            rec.brushes.iter().all(|b| matches!(b, Brush::Solid(_))),
            "the verb and the timer are solid"
        );
        assert_eq!(rec.brushes.len(), 2, "the verb and the timer");
    }

    /// Each variant carries upstream's own default status text.
    #[test]
    fn each_variant_carries_its_own_default_label() {
        assert_eq!(
            LoadingStatesVariant::Dots.default_label(),
            DEFAULT_DOTS_LABEL
        );
        assert_eq!(
            LoadingStatesVariant::Progress.default_label(),
            DEFAULT_PROGRESS_LABEL
        );
        assert_eq!(LoadingStatesVariant::Shimmer.default_label(), DEFAULT_LABEL);
        assert_eq!(
            LoadingStatesVariant::Reasoning.default_label(),
            DEFAULT_LABEL
        );

        let widget = laid_out(&loading_states().label("Compiling"));
        assert_eq!(widget.display(), "Compiling");
    }

    /// A variant swap restarts the cycle clock and relayouts; a redundant
    /// rebuild changes nothing.
    #[test]
    fn a_variant_swap_restarts_the_cycle_and_a_redundant_rebuild_does_not() {
        let shimmer = loading_states();
        let progress = loading_states().variant(LoadingStatesVariant::Progress);
        let mut widget = laid_out(&shimmer);
        painted(&mut widget, 400, None);
        let mut next_id = 0u64;

        let flags = View::<()>::rebuild(
            &shimmer,
            &shimmer,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert_eq!(flags, ChangeFlags::NONE);

        let flags = View::<()>::rebuild(
            &progress,
            &shimmer,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert_eq!(widget.variant(), LoadingStatesVariant::Progress);
        assert!(
            widget.started.is_none(),
            "the new variant's cycle is timed from its own first frame"
        );
    }

    /// The stagger helper never runs a part backwards before its delay lands.
    #[test]
    fn a_staggered_part_waits_at_the_cycle_start_until_its_delay_lands() {
        let period = Duration::from_millis(1000);
        let delay = Duration::from_millis(400);
        assert_eq!(staggered_cycle(Duration::ZERO, period, delay), 0.0);
        assert_eq!(
            staggered_cycle(Duration::from_millis(400), period, delay),
            0.0
        );
        let mid = staggered_cycle(Duration::from_millis(900), period, delay);
        assert!((mid - 0.5).abs() < 1e-9, "half a cycle in: {mid}");
        // A zero period has no cycle rather than dividing by zero.
        assert_eq!(cycle(Duration::from_secs(1), Duration::ZERO), 0.0);
    }

    /// The eased keyframe read is anchored exactly at every keyframe and never
    /// leaves the segment it is inside.
    #[test]
    fn eased_keyframes_stay_anchored_at_every_keyframe() {
        let frames = [0.28, 1.0, 0.28];
        assert!((eased_keyframes(&frames, 0.0, EASE_IN_OUT) - 0.28).abs() < 1e-9);
        assert!((eased_keyframes(&frames, 0.5, EASE_IN_OUT) - 1.0).abs() < 1e-9);
        assert!((eased_keyframes(&frames, 1.0, EASE_IN_OUT) - 0.28).abs() < 1e-9);
        // Within a segment the read is monotonic between its two keyframes,
        // and never leaves them — the property a *global* easing over the whole
        // timeline would break by pulling one segment past its own endpoint.
        let quarter = eased_keyframes(&frames, 0.25, EASE_IN_OUT);
        assert!(
            quarter > 0.28 && quarter < 1.0,
            "inside segment 0: {quarter}"
        );
        let three_quarters = eased_keyframes(&frames, 0.75, EASE_IN_OUT);
        assert!(
            three_quarters > 0.28 && three_quarters < 1.0,
            "inside segment 1: {three_quarters}"
        );
        // A degenerate array falls through to the linear read.
        assert_eq!(eased_keyframes(&[7.0], 0.5, EASE_IN_OUT), 7.0);
        assert_eq!(eased_keyframes(&[], 0.5, EASE_IN_OUT), 0.0);
    }

    // ---- Typeface: the status text follows the live theme, the timer is mono -

    use crate::agents::code_block::mixed_face_probe::{
        assert_mixed_follows_a_live_family_swap, assert_paints_geist_beside_mono,
    };
    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// The shimmering status line.
    fn shimmer_probe(_: &mut ()) -> LoadingStatesView {
        loading_states().label("Thinking")
    }

    /// The reasoning phrase beside its dot row.
    fn reasoning_probe(_: &mut ()) -> LoadingStatesView {
        loading_states()
            .variant(LoadingStatesVariant::Reasoning)
            .phrases(["Reading the diff", "Checking the tests"])
    }

    /// The progress verb beside the elapsed timer.
    fn progress_probe(_: &mut ()) -> LoadingStatesView {
        loading_states()
            .variant(LoadingStatesVariant::Progress)
            .label("Building")
            .elapsed_seconds(12.0)
    }

    /// The elapsed timer, the one explicit mono run [`progress_probe`] paints.
    const PROGRESS_MONO_RUNS: usize = 1;

    #[test]
    fn status_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the shimmer", shimmer_probe, BOX);
        assert_paints_only_in_geist("the reasoning line", reasoning_probe, BOX);
        assert_paints_geist_beside_mono(
            "the progress row",
            progress_probe,
            BOX,
            PROGRESS_MONO_RUNS,
        );
    }

    #[test]
    fn status_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the shimmer", shimmer_probe, BOX);
        assert_follows_a_live_family_swap("the reasoning line", reasoning_probe, BOX);
        assert_mixed_follows_a_live_family_swap(
            "the progress row",
            progress_probe,
            BOX,
            PROGRESS_MONO_RUNS,
        );
    }
}
