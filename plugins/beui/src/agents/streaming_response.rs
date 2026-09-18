//! Ports beUI's `streaming-response` agent-interface part.
//!
//! **Source:** `components/agents/streaming-response.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `streaming-response`: *"A stable response surface with completion
//! actions, rendered content, and an expandable source summary."*
//!
//! # What this component is
//!
//! The **surface streamed text lands on**: a wrapped body run that grows as
//! tokens arrive, with each appended run fading in and everything already on
//! screen left alone, plus a shimmering cursor while the stream is live.
//!
//! # Append-driven reveal, not per-character cells
//!
//! The obvious shape — one animatable cell per character, which
//! [`motion::chars`](crate::motion::chars) already provides — is the wrong one
//! here, and deliberately not used: a response is paragraphs, not a heading, and
//! splitting it per grapheme throws away kerning, line-breaking and the shape
//! cache for text nobody animates *individually*. Instead:
//!
//! 1. The whole content is shaped as **one wrapped run**, so line-breaking is
//!    the text stack's.
//! 2. The widget remembers how many glyphs of that run are **settled** — the
//!    prefix that has already finished revealing.
//! 3. An append leaves the settled count where it is and fades only the glyphs
//!    past it, over [`STREAM_REVEAL_MS`].
//!
//! The consequence is the property the whole design exists for: **an append
//! never re-animates prior content**. A second append arriving mid-fade settles
//! the first one outright (its glyphs join the settled prefix) rather than
//! restarting it.
//!
//! ## Why the boundary is a glyph count
//!
//! Shaping is the only thing that knows how many glyphs a string produces, and
//! `layout` is the only pass that may shape. The count is therefore taken from
//! the run that is *already shaped* at the moment the append arrives — which is
//! precisely the previous content's run — so no second shaping pass and no
//! character-to-glyph mapping is needed. A re-wrap at the boundary can move a
//! word onto the next line; the glyph count is unchanged by that, because
//! line-breaking does not add or remove glyphs.
//!
//! # Degradations against upstream
//!
//! - **No completion action row.** Upstream's copy / retry / thumbs cluster
//!   needs a clipboard seam the framework does not publish and four Lucide
//!   icons; a caller composes [`crate::components::button`]s under the response
//!   instead.
//! - **No sources disclosure.** Upstream folds
//!   [`citations`](super::citations)'s `CitationStack`/`CitationList` into its
//!   footer. That is the `citations` slug's own component, and it composes above
//!   this one rather than inside it.
//! - **The reveal ramp is this port's own.** Upstream does not animate the
//!   arrival of streamed text at all (React simply re-renders); a token boundary
//!   that pops is what a native surface makes visible and a DOM one hides, so
//!   the port fades it over [`STREAM_REVEAL_MS`] — the same 120 ms
//!   `message-bubble`'s content reveal uses.
//! - **The cursor is a block, not a caret glyph.** Upstream renders no cursor;
//!   the shimmer here is the port's own "still streaming" affordance, sized from
//!   the type scale.
//! - **Markdown is the caller's.** Upstream takes `children` and styles `<pre>`,
//!   `<code>`, `<ul>` and friends through a class list. This component takes a
//!   plain string; a rendered-markdown tree is a different component's job.

use std::time::Duration;

use frust::authoring::scene::GlyphRun;
use frust::authoring::text::{TextContext, TextLayout, TextStyle};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Role, SemanticsCtx,
    Size, TickClass, View, Widget,
};
use frust::{FrameTime, Theme};
use kurbo::Point;
use peniko::{Brush, Color};

use crate::motion::Ramp;
use crate::press::Lane;
use crate::style::{self, scale_alpha};
use crate::text::{ThemeTextType, themed_style};
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::EASE_OUT;

/// How long one appended run takes to fade in, in ms — the port's own ramp,
/// matching [`message_bubble`](super::message_bubble)'s content reveal.
pub const STREAM_REVEAL_MS: u64 = 120;

/// One full cursor shimmer cycle, in ms.
pub const CURSOR_PERIOD: Duration = Duration::from_millis(900);

/// The cursor's width, in logical px.
pub const CURSOR_WIDTH: f64 = 2.0;

/// The cursor's height as a fraction of the type size — an em box's rough
/// cap height, which is what a text caret spans.
const CURSOR_HEIGHT_FACTOR: f64 = 1.0;

/// How far past the last glyph's origin the cursor sits, as a fraction of the
/// type size.
const CURSOR_ADVANCE_FACTOR: f64 = 0.62;

/// How far above the baseline the cursor's top sits, as a fraction of the type
/// size.
const CURSOR_ASCENT_FACTOR: f64 = 0.78;

/// The cursor's dimmest alpha through its shimmer.
const CURSOR_MIN_ALPHA: f32 = 0.25;

/// The cursor's brightest alpha through its shimmer.
const CURSOR_MAX_ALPHA: f32 = 0.9;

/// The cursor's fixed alpha under `reduce_motion` — present, but not pulsing.
const CURSOR_REDUCED_ALPHA: f32 = 0.6;

/// Alpha the body is painted at (`text-foreground/90`).
const BODY_ALPHA: f32 = 0.9;

/// The body's line height, in logical px (`leading-6`).
pub const STREAM_LINE_HEIGHT: f64 = 24.0;

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.foreground;
/// Unthemed fallback error ink (beUI light `--destructive`).
const FALLBACK_ERROR: Color = BEUI_LIGHT.destructive;

/// Where a response is in its lifecycle — upstream's
/// `StreamingResponseStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamingResponseStatus {
    /// Tokens are still arriving: the cursor shimmers and the surface reports
    /// itself busy.
    #[default]
    Streaming,
    /// The response is finished.
    Complete,
    /// The response failed; the body paints in the error role.
    Error,
}

impl StreamingResponseStatus {
    /// Every status, in upstream's own union order.
    pub const ALL: [StreamingResponseStatus; 3] = [
        StreamingResponseStatus::Streaming,
        StreamingResponseStatus::Complete,
        StreamingResponseStatus::Error,
    ];

    /// Whether this status still shows a cursor (`aria-busy`).
    pub const fn is_streaming(self) -> bool {
        matches!(self, StreamingResponseStatus::Streaming)
    }
}

/// A declarative beUI streamed-response surface. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::agents::streaming_response::{
///     StreamingResponseStatus, streaming_response,
/// };
///
/// // Each rebuild hands in the whole text so far; the widget reveals only
/// // what is new.
/// let live = streaming_response("The answer is ");
/// let done = streaming_response("The answer is 42.")
///     .status(StreamingResponseStatus::Complete);
/// ```
pub struct StreamingResponseView {
    content: String,
    status: StreamingResponseStatus,
    size: f32,
    color: Option<Color>,
}

/// Create a response surface showing `content`, still
/// [`Streaming`](StreamingResponseStatus::Streaming).
pub fn streaming_response(content: impl Into<String>) -> StreamingResponseView {
    StreamingResponseView {
        content: content.into(),
        status: StreamingResponseStatus::default(),
        size: style::TEXT_SM as f32,
        color: None,
    }
}

impl StreamingResponseView {
    /// Report `status` instead of
    /// [`Streaming`](StreamingResponseStatus::Streaming).
    pub fn status(mut self, status: StreamingResponseStatus) -> Self {
        self.status = status;
        self
    }

    /// Set the body's type size, in logical px (default [`style::TEXT_SM`]).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0) as f32;
        self
    }

    /// Paint the body in `color` instead of the theme's resolved ink. An
    /// [`Error`](StreamingResponseStatus::Error) response still uses the error
    /// role unless this is set.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// The text this view carries.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// The retained widget for a [`StreamingResponseView`].
pub struct StreamingResponseWidget {
    content: String,
    status: StreamingResponseStatus,
    size: f32,
    color: Option<Color>,
    /// The wrapped body run, plus what it was shaped from (text, style, width).
    run: Option<TextLayout>,
    shaped: Option<(String, TextStyle, f64)>,
    /// How many glyphs of the shaped run have finished revealing. Everything
    /// past this is the run currently fading in.
    settled_glyphs: usize,
    /// The appended run's fade, `0.0` invisible .. `1.0` settled.
    reveal: Lane,
    /// The frame this widget first painted on — the cursor's cycle clock.
    started: Option<FrameTime>,
}

impl StreamingResponseWidget {
    /// The whole text this surface carries.
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Where the response is in its lifecycle.
    pub fn status(&self) -> StreamingResponseStatus {
        self.status
    }

    /// How many glyphs the shaped run holds in total — `0` before the first
    /// layout.
    pub fn total_glyphs(&self) -> usize {
        self.run
            .as_ref()
            .map_or(0, |run| glyph_count(&run.to_scene_runs(Point::ORIGIN)))
    }

    /// How many glyphs are settled: painted at full ink, never re-animated.
    pub fn settled_glyphs(&self) -> usize {
        self.settled_glyphs
    }

    /// How far through its fade the appended run is; `1.0` when nothing is
    /// pending.
    pub fn reveal_progress(&self) -> f64 {
        self.reveal.value()
    }

    /// The body's shaping style, in the theme's `body_medium` family. `layout`
    /// keys the shaped run on it, so a theme swap that changes the family
    /// reshapes the body.
    fn body_style(&self, theme: Option<&Theme>, ink: Color) -> TextStyle {
        let style = TextStyle {
            line_height: frust::authoring::text::LineHeight::Absolute(STREAM_LINE_HEIGHT as f32),
            ..TextStyle::new(self.size, ink)
        };
        themed_style(style, ThemeTextType::BodyMedium, theme)
    }

    /// The body's ink: the explicit override, else the error role for a failed
    /// response, else the theme's `on_surface` at `text-foreground/90`.
    fn ink(&self, theme: Option<&Theme>) -> Color {
        if let Some(color) = self.color {
            return color;
        }
        let base = match (self.status, theme) {
            (StreamingResponseStatus::Error, Some(theme)) => theme.scheme().error,
            (StreamingResponseStatus::Error, None) => FALLBACK_ERROR,
            (_, Some(theme)) => theme.scheme().on_surface,
            (_, None) => FALLBACK_INK,
        };
        scale_alpha(base, BODY_ALPHA)
    }

    /// Settle whatever is currently fading, then begin a fade for everything
    /// past it — the one operation an append performs.
    ///
    /// The settled boundary is read from the run *as currently shaped*, which
    /// is the previous content's; see the [module docs](self).
    fn append(&mut self) {
        self.settled_glyphs = self.total_glyphs();
        self.reveal.snap();
        self.reveal.retarget(0.0);
        self.reveal.snap();
        self.reveal.retarget(1.0);
    }

    /// Restart the reveal from nothing — a content replacement rather than an
    /// append.
    fn restart(&mut self) {
        self.settled_glyphs = 0;
        self.reveal.retarget(0.0);
        self.reveal.snap();
        self.reveal.retarget(1.0);
    }
}

/// How many glyphs a set of scene runs holds in total.
fn glyph_count(runs: &[GlyphRun]) -> usize {
    runs.iter().map(|run| run.glyphs.len()).sum()
}

/// Whether `next` is `prev` plus more text — the append test the whole reveal
/// hangs on. An unchanged string is *not* an append (nothing arrived).
fn is_append(prev: &str, next: &str) -> bool {
    next.len() > prev.len() && next.starts_with(prev)
}

/// Split `runs` at global glyph index `boundary`, returning the runs to paint
/// settled and the runs to paint at the reveal's alpha.
///
/// A run straddling the boundary is split in two, sharing its font, size and
/// transform — which is what keeps the boundary at a glyph rather than at a
/// whole shaped run.
fn split_runs(runs: Vec<GlyphRun>, boundary: usize) -> (Vec<GlyphRun>, Vec<GlyphRun>) {
    let mut settled = Vec::new();
    let mut pending = Vec::new();
    let mut seen = 0usize;
    for run in runs {
        let len = run.glyphs.len();
        if seen + len <= boundary {
            seen += len;
            settled.push(run);
        } else if seen >= boundary {
            seen += len;
            pending.push(run);
        } else {
            let cut = boundary - seen;
            seen += len;
            let mut head = run.clone();
            head.glyphs.truncate(cut);
            let mut tail = run;
            tail.glyphs.drain(..cut);
            settled.push(head);
            pending.push(tail);
        }
    }
    (settled, pending)
}

/// Where the cursor sits, given the shaped runs and the type size: just past
/// the last glyph of the last line, or at `origin` when there is no text yet.
fn cursor_at(runs: &[GlyphRun], origin: Point, size: f64) -> Point {
    let mut best: Option<(f32, f32)> = None;
    for run in runs {
        let offset = run.transform.translation();
        for glyph in &run.glyphs {
            let (x, y) = (glyph.x + offset.x as f32, glyph.y + offset.y as f32);
            best = match best {
                // The last glyph in reading order: lowest line first, then
                // furthest along it.
                Some((bx, by)) if (y, x) <= (by, bx) => Some((bx, by)),
                _ => Some((x, y)),
            };
        }
    }
    match best {
        Some((x, y)) => Point::new(
            x as f64 + size * CURSOR_ADVANCE_FACTOR,
            y as f64 - size * CURSOR_ASCENT_FACTOR,
        ),
        None => origin,
    }
}

/// The cursor's alpha at `phase` of its cycle — a symmetric fade between
/// [`CURSOR_MIN_ALPHA`] and [`CURSOR_MAX_ALPHA`].
fn cursor_alpha(phase: f64) -> f32 {
    let triangle = 1.0 - (phase * 2.0 - 1.0).abs();
    CURSOR_MIN_ALPHA + (CURSOR_MAX_ALPHA - CURSOR_MIN_ALPHA) * triangle as f32
}

impl<State: 'static> View<State> for StreamingResponseView {
    type Element = StreamingResponseWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> StreamingResponseWidget {
        StreamingResponseWidget {
            content: self.content.clone(),
            status: self.status,
            size: self.size,
            color: self.color,
            run: None,
            shaped: None,
            settled_glyphs: 0,
            // The first content is not an append: it arrives settled, so a
            // transcript of finished answers plays nothing.
            reveal: Lane::at_rest(
                Ramp::eased(Duration::from_millis(STREAM_REVEAL_MS), EASE_OUT),
                1.0,
            ),
            started: None,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut StreamingResponseWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.content != self.content {
            if is_append(&element.content, &self.content) {
                element.append();
            } else {
                element.restart();
            }
            element.content = self.content.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.status != self.status {
            element.status = self.status;
            flags |= ChangeFlags::PAINT;
        }
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.color != self.color {
            element.color = self.color;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, _element: &mut StreamingResponseWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for StreamingResponseWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let ink = self.ink(theme);
        let style = self.body_style(theme, ink);
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            f64::INFINITY
        };

        let key = (self.content.clone(), style.clone(), width);
        if self.shaped.as_ref() != Some(&key) {
            let max_width = width.is_finite().then_some(width as f32);
            let text_ctx = ctx.text_context::<TextContext>();
            self.run = Some(text_ctx.layout(&self.content, &style, max_width));
            self.shaped = Some(key);
        }
        let measured = self.run.as_ref().map_or(Size::ZERO, TextLayout::size);
        // A streaming surface reserves at least one line, so the cursor has
        // somewhere to sit before the first token lands.
        bc.constrain(Size::new(
            measured.width,
            measured.height.max(STREAM_LINE_HEIGHT),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, ink) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                self.ink(theme),
            )
        };
        let now = ctx.frame_time();
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let origin = ctx.origin();

        if reduce_motion {
            self.reveal.snap();
        }
        let mut owes_frame = self.reveal.advance(now);
        let reveal = self.reveal.value().clamp(0.0, 1.0);

        let runs = self
            .run
            .as_ref()
            .map(|run| run.to_scene_runs(origin))
            .unwrap_or_default();
        let cursor = cursor_at(&runs, origin, self.size as f64);
        let (settled, pending) = split_runs(runs, self.settled_glyphs);

        for mut run in settled {
            run.brush = Brush::Solid(ink);
            scene.draw_glyph_run(run);
        }
        if reveal > 0.0 {
            let faded = scale_alpha(ink, reveal as f32);
            for mut run in pending {
                run.brush = Brush::Solid(faded);
                scene.draw_glyph_run(run);
            }
        }

        if self.status.is_streaming() {
            let alpha = if reduce_motion {
                CURSOR_REDUCED_ALPHA
            } else {
                cursor_alpha(cycle(elapsed, CURSOR_PERIOD))
            };
            let height = self.size as f64 * CURSOR_HEIGHT_FACTOR;
            scene.fill_rounded_rect(
                cursor,
                Size::new(CURSOR_WIDTH, height),
                CURSOR_WIDTH / 2.0,
                scale_alpha(ink, alpha),
            );
            if !reduce_motion {
                // A perpetual decorative loop, never a transition.
                ctx.request_frame_class(TickClass::CosmeticLoop);
                owes_frame = false;
            }
        }
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // `aria-live="polite"` over the whole content — the resolved text, not
        // the partially-faded paint.
        let content = self.content.clone();
        ctx.push_node(Role::Status, |node| {
            node.set_label(content.as_str());
        });
    }
}

/// `elapsed` folded into `[0, 1)` of a `period`-long loop.
fn cycle(elapsed: Duration, period: Duration) -> f64 {
    if period.is_zero() {
        return 0.0;
    }
    let turns = elapsed.as_secs_f64() / period.as_secs_f64();
    turns - turns.floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    /// The box every response test lays itself into.
    const BOX: Size = Size::new(260.0, 200.0);

    /// Records each glyph run's brush and glyph count, plus the cursor block.
    #[derive(Default)]
    struct Recorder {
        runs: Vec<(Brush, usize)>,
        rounded: Vec<(Point, Size, Color)>,
    }

    impl Recorder {
        /// Total glyphs painted at full ink (alpha unchanged from `ink`).
        fn glyphs_at(&self, alpha: f32) -> usize {
            self.runs
                .iter()
                .filter(|(brush, _)| match brush {
                    Brush::Solid(color) => (color.components[3] - alpha).abs() < 1e-4,
                    _ => false,
                })
                .map(|(_, count)| count)
                .sum()
        }

        /// Every distinct alpha a glyph run was painted at.
        fn alphas(&self) -> Vec<f32> {
            self.runs
                .iter()
                .filter_map(|(brush, count)| match brush {
                    Brush::Solid(color) if *count > 0 => Some(color.components[3]),
                    _ => None,
                })
                .collect()
        }
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.runs.push((run.brush.clone(), run.glyphs.len()));
        }
    }

    fn laid_out(view: &StreamingResponseView) -> StreamingResponseWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut StreamingResponseWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
    }

    fn painted(
        widget: &mut StreamingResponseWidget,
        ms: u64,
        theme: Option<&Theme>,
    ) -> (Recorder, bool, Option<TickClass>) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame(), ctx.frame_class())
    }

    /// Hand `widget` a longer string, exactly as a stream's next rebuild would.
    fn stream(widget: &mut StreamingResponseWidget, content: &str) {
        let view = streaming_response(content);
        let mut next_id = 0u64;
        View::<()>::rebuild(&view, &view, widget, &mut BuildCtx::new(&mut next_id));
        relayout(widget);
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// The append test the whole reveal hangs on: only a strictly longer
    /// string with the old one as its prefix counts.
    #[test]
    fn only_a_strict_prefix_extension_is_an_append() {
        assert!(is_append("The answer", "The answer is"));
        assert!(is_append("", "T"));
        assert!(
            !is_append("The answer", "The answer"),
            "unchanged is not new"
        );
        assert!(!is_append("The answer", "An answer"), "a replacement");
        assert!(!is_append("The answer is", "The answer"), "a truncation");
    }

    /// A response built with its whole text arrives settled — no fade, no
    /// pending run — so a transcript of finished answers plays nothing.
    #[test]
    fn the_first_content_arrives_settled() {
        let mut widget = laid_out(
            &streaming_response("The answer is 42.").status(StreamingResponseStatus::Complete),
        );
        assert_eq!(widget.reveal_progress(), 1.0);
        assert_eq!(widget.settled_glyphs(), 0, "nothing has settled *yet*");

        let (rec, needs_frame, _) = painted(&mut widget, 0, None);
        assert!(!needs_frame, "a complete response owes no frames");
        // Everything painted is at the reveal's own full alpha.
        let alphas = rec.alphas();
        assert!(!alphas.is_empty());
        assert!(alphas.iter().all(|a| (*a - BODY_ALPHA).abs() < 1e-4));
        assert!(rec.rounded.is_empty(), "no cursor once complete");
    }

    /// The heart of the component: an append fades only the new glyphs, and
    /// everything already on screen keeps painting at full ink.
    #[test]
    fn an_append_fades_only_the_new_glyphs() {
        let mut widget = laid_out(&streaming_response("The answer"));
        painted(&mut widget, 0, None);
        let before = widget.total_glyphs();
        assert!(before > 0);

        stream(&mut widget, "The answer is 42.");
        assert_eq!(
            widget.settled_glyphs(),
            before,
            "the prior run settled at the append"
        );
        assert_eq!(
            widget.reveal_progress(),
            0.0,
            "the new run starts invisible"
        );

        // Mid-fade: two alphas on screen, the settled one full and the pending
        // one partial.
        painted(&mut widget, 100, None);
        let (rec, ..) = painted(&mut widget, 160, None);
        let mid = widget.reveal_progress();
        assert!(mid > 0.0 && mid < 1.0, "mid-fade: {mid}");
        assert_eq!(
            rec.glyphs_at(BODY_ALPHA),
            before,
            "the prior text is untouched at full ink"
        );
        let total = widget.total_glyphs();
        assert!(total > before);
        let faded: usize = rec
            .runs
            .iter()
            .filter(|(brush, _)| match brush {
                Brush::Solid(color) => color.components[3] < BODY_ALPHA - 1e-4,
                _ => false,
            })
            .map(|(_, count)| count)
            .sum();
        assert_eq!(faded, total - before, "exactly the appended run fades");

        // Settled: one alpha again.
        painted(&mut widget, 5_000, None);
        assert_eq!(widget.reveal_progress(), 1.0);
    }

    /// A second append arriving mid-fade settles the first one outright rather
    /// than restarting it — the "never re-animates prior content" rule under
    /// its hardest case.
    #[test]
    fn a_second_append_settles_the_first_instead_of_restarting_it() {
        let mut widget = laid_out(&streaming_response("The"));
        painted(&mut widget, 0, None);
        let _first = widget.total_glyphs();

        stream(&mut widget, "The answer");
        painted(&mut widget, 10, None);
        painted(&mut widget, 40, None);
        let partway = widget.reveal_progress();
        assert!(partway > 0.0 && partway < 1.0, "still fading: {partway}");
        let second = widget.total_glyphs();

        // A second token arrives before the first finished fading.
        stream(&mut widget, "The answer is 42.");
        assert_eq!(
            widget.settled_glyphs(),
            second,
            "everything up to the second append is now settled"
        );
        let (rec, ..) = painted(&mut widget, 50, None);
        assert_eq!(
            rec.glyphs_at(BODY_ALPHA),
            second,
            "the first append is at full ink, not re-faded"
        );
    }

    /// A content *replacement* is not an append: the whole run fades from
    /// nothing.
    #[test]
    fn a_replacement_restarts_the_reveal_from_nothing() {
        let mut widget = laid_out(&streaming_response("The answer"));
        painted(&mut widget, 0, None);

        stream(&mut widget, "A different answer entirely");
        assert_eq!(widget.settled_glyphs(), 0);
        assert_eq!(widget.reveal_progress(), 0.0);

        let (rec, ..) = painted(&mut widget, 10, None);
        assert_eq!(rec.glyphs_at(BODY_ALPHA), 0, "nothing is settled");
    }

    /// The cursor shows only while streaming, shimmers on a perpetual
    /// decorative loop, and holds a fixed alpha under reduced motion.
    #[test]
    fn the_cursor_shimmers_only_while_streaming() {
        let mut widget = laid_out(&streaming_response("The answer"));
        painted(&mut widget, 0, None);
        let (rec, needs_frame, class) = painted(&mut widget, 200, None);
        assert!(needs_frame);
        assert_eq!(class, Some(TickClass::CosmeticLoop));
        assert_eq!(rec.rounded.len(), 1, "one cursor block");
        let (_, size, first) = rec.rounded[0];
        assert_eq!(size.width, CURSOR_WIDTH);

        let (rec, ..) = painted(&mut widget, 650, None);
        let (_, _, later) = rec.rounded[0];
        assert!(
            (first.components[3] - later.components[3]).abs() > 1e-4,
            "the cursor's alpha moves across the cycle"
        );

        let theme = reduced();
        let (rec, needs_frame, _) = painted(&mut widget, 700, Some(&theme));
        assert!(!needs_frame, "reduced motion stops the shimmer");
        let (_, _, calm) = rec.rounded[0];
        assert!((calm.components[3] - BODY_ALPHA * CURSOR_REDUCED_ALPHA).abs() < 1e-4);
    }

    /// Every status is constructible; only `Streaming` is busy, and `Error`
    /// paints in the error role.
    #[test]
    fn each_status_paints_its_own_ink() {
        assert_eq!(StreamingResponseStatus::ALL.len(), 3);
        for status in StreamingResponseStatus::ALL {
            let mut widget = laid_out(&streaming_response("hi").status(status));
            let (rec, ..) = painted(&mut widget, 0, None);
            assert_eq!(widget.status(), status);
            assert_eq!(
                rec.rounded.is_empty(),
                !status.is_streaming(),
                "{status:?} cursor presence"
            );
        }

        let theme = crate::theme();
        let errored = laid_out(&streaming_response("nope").status(StreamingResponseStatus::Error));
        assert_eq!(
            errored.ink(Some(&theme)),
            scale_alpha(theme.scheme().error, BODY_ALPHA)
        );
        let normal = laid_out(&streaming_response("ok"));
        assert_eq!(
            normal.ink(Some(&theme)),
            scale_alpha(theme.scheme().on_surface, BODY_ALPHA)
        );
        // An explicit colour wins over both.
        let forced = laid_out(&streaming_response("ok").color(Color::from_rgb8(1, 2, 3)));
        assert_eq!(forced.ink(Some(&theme)), Color::from_rgb8(1, 2, 3));
    }

    /// The glyph-boundary split is exact, including a run that straddles it.
    #[test]
    fn splitting_runs_cuts_exactly_at_the_glyph_boundary() {
        let widget = laid_out(&streaming_response("The answer is 42."));
        let runs = widget
            .run
            .as_ref()
            .expect("shaped")
            .to_scene_runs(Point::ORIGIN);
        let total = glyph_count(&runs);
        assert!(total > 4);

        for boundary in [0, 1, total / 2, total - 1, total] {
            let (settled, pending) = split_runs(runs.clone(), boundary);
            assert_eq!(glyph_count(&settled), boundary, "at {boundary}");
            assert_eq!(glyph_count(&pending), total - boundary, "at {boundary}");
        }

        // Past the end everything is settled rather than panicking.
        let (settled, pending) = split_runs(runs, total + 10);
        assert_eq!(glyph_count(&settled), total);
        assert!(pending.is_empty());
    }

    /// The cursor's shimmer is a symmetric triangle between its two alphas.
    #[test]
    fn the_cursor_alpha_is_a_symmetric_triangle() {
        assert!((cursor_alpha(0.0) - CURSOR_MIN_ALPHA).abs() < 1e-6);
        assert!((cursor_alpha(0.5) - CURSOR_MAX_ALPHA).abs() < 1e-6);
        assert!((cursor_alpha(1.0) - CURSOR_MIN_ALPHA).abs() < 1e-6);
        assert!((cursor_alpha(0.25) - cursor_alpha(0.75)).abs() < 1e-6);
        assert_eq!(cycle(Duration::from_secs(1), Duration::ZERO), 0.0);
    }

    /// An empty response still reserves a line, so the cursor has somewhere to
    /// sit before the first token lands.
    #[test]
    fn an_empty_response_still_reserves_a_line_for_its_cursor() {
        let mut widget = laid_out(&streaming_response(""));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        let size = widget.layout(&mut layout, &BoxConstraints::loose(BOX));
        assert!(size.height >= STREAM_LINE_HEIGHT);

        let (rec, ..) = painted(&mut widget, 0, None);
        assert_eq!(rec.rounded.len(), 1, "the cursor is there from the start");
        assert_eq!(rec.rounded[0].0, Point::ORIGIN);
    }

    /// A redundant rebuild changes nothing — no reveal restarts on a rebuild
    /// that carries the same text.
    #[test]
    fn a_redundant_rebuild_does_not_restart_the_reveal() {
        let view = streaming_response("The answer");
        let mut widget = laid_out(&view);
        painted(&mut widget, 0, None);
        painted(&mut widget, 5_000, None);
        let settled = widget.settled_glyphs();

        let mut next_id = 0u64;
        let flags =
            View::<()>::rebuild(&view, &view, &mut widget, &mut BuildCtx::new(&mut next_id));
        assert_eq!(flags, ChangeFlags::NONE);
        assert_eq!(widget.settled_glyphs(), settled);
        assert_eq!(widget.reveal_progress(), 1.0);
    }

    // ---- Typeface: the body follows the live theme ---------------------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// A body long enough to wrap in [`BOX`], so more than one line paints.
    fn probe_view(_: &mut ()) -> StreamingResponseView {
        streaming_response("The build passed. Two warnings remain in the checkout module.")
            .status(StreamingResponseStatus::Complete)
    }

    #[test]
    fn the_body_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the response body", probe_view, BOX);
    }

    #[test]
    fn the_body_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the response body", probe_view, BOX);
    }
}
