//! Glyph command palette: the signature
//! amber-ring overlay — a prompt-prefixed search field over a filtered result
//! list, the Glyph recipe over the existing navigator transparent-push modal
//! plumbing.
//!
//! # Controlled component (the app owns the query and the filtering)
//!
//! The palette is a **controlled** component: it takes the current query string
//! plus the already-filtered `items: Vec<`[`PaletteItem`]`>` and streams input
//! changes back through [`on_query`](CommandPaletteView::on_query). There is
//! **no internal fuzzy engine** — filtering `items` down for a given query is
//! the app's job (v1). A row tap fires [`on_select`](CommandPaletteView::on_select)
//! with the row's index into the *currently supplied* `items`.
//!
//! # Navigator-modal architecture (reuse, don't fork)
//!
//! Like [`crate::glyph::dialog`], the palette is pushed as a **transparent
//! navigator page** via [`NavigatorController::push_transparent_for_result`] —
//! the navigator's modal contract routes input only to the top page and threads
//! pointer capture / focus / IME through the page pod, and
//! [`show_command_palette`] wraps the push and wires dismissal to
//! `controller.pop()`. No nav file is edited here.
//!
//! # Enter/exit staging (widget-internal — snappier than the dialog)
//!
//! The palette drives its own enter/exit animation from `PaintCtx::frame_time`
//! (see [`crate::glyph::dialog`]'s rationale): the **panel** scales `0.96 → 1.0`
//! over the spatial `fast` duration (150ms) while the **scrim** cross-fades on
//! its own progress; exit reverses over the even-faster `instant` duration
//! (100ms) — snappier by design than the dialog (the design system's rule:
//! "exits always faster than entrances"). A scrim/Escape cancel flips the widget into its exit
//! phase and fires the state-free close callback only once the exit completes
//! (from paint — sound because `NavigatorController::pop` merely enqueues an op).
//!
//! # Focus lands in the input on open, never on a scrim-bound dismiss
//!
//! Frust has no focus-on-*appear* seam yet (focus can only be claimed from an
//! event pass — see [`crate::material::dialog`]'s auto-focus note) — so "on
//! open" is realized at the palette's **first event pass**, routing a synthetic
//! tap into its [`TextInput`](crate::TextInput) child so typing flows into the
//! field with no explicit tap. The one-shot is guarded against the outside-tap
//! dismiss flash a naive "first event" trigger produces: an outside `Down` is
//! itself the start of a scrim-dismiss gesture, so the guard skips the
//! autofocus routing exactly when the first event *is* that `Down` (checked by
//! [`CommandPaletteWidget::is_scrim_down`]) — the keyboard never flashes up
//! only to be dismissed by the same gesture's `Up`. Any other first event
//! (including a tap on the input/a row) still claims focus immediately.
//! Dismissal tears down the page, which clears the field's focus/IME per the
//! suppressing-container contract.
//!
//! # Dismissable, and back-press parity
//!
//! [`dismissable(bool)`](CommandPaletteView::dismissable) (default `true`)
//! governs every non-explicit dismissal path together: scrim tap, Escape, and
//! a routed back press. `false` makes the palette a modal barrier — scrim tap
//! and Escape become no-ops, and [`show_command_palette`] pushes the page with
//! [`BackPolicy::Veto`] (a back press is consumed but changes nothing). `true`
//! (the default) keeps scrim/Escape dismissing as before, and the page pushes
//! with [`BackPolicy::DismissAnimated`]: a routed back press bumps a shared
//! dismiss-signal generation cell rather than popping the stack directly; the
//! widget compares it against its last-seen value once per paint (see
//! [`BackPolicy`]'s observation seam) and stages the same animated
//! [`begin_exit`](CommandPaletteWidget::begin_exit) a scrim/Escape cancel does,
//! popping itself via [`on_close`](CommandPaletteView::on_close) only once the
//! exit completes. The explicit close paths (`on_close`, a row's `on_select`)
//! always work regardless of `dismissable`.
//!
//! # Only one floating layer
//!
//! At most one palette/overlay at a time is the app's/navigator's concern, not
//! this widget's (mirrors [`crate::glyph::dialog`]).

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust_core::accesskit::Role;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Curve, EventCtx, EventResult,
    FrameTime, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, PointerButton,
    PointerEvent, PointerPhase, SemanticsCtx, View, Widget, any,
};
use frust_text::{
    FontFamily, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust_theme::Theme;
use kurbo::{Affine, Point, Rect, RoundedRect, Shape, Size, Vec2};
use peniko::{Brush, Color};

use crate::Timing;
use crate::nav::navigator::{BackPolicy, NavigatorController, PopResult, PushOptions};
use crate::nav::transition::{TransitionDriver, TransitionSpec, make_driver};
use crate::text_input;

/// Panel padding on all four edges, in logical px.
const PANEL_PAD: f64 = 8.0;
/// The prompt-prefixed input row height, in logical px.
const INPUT_ROW_H: f64 = 40.0;
/// A result row's height, in logical px.
const ROW_H: f64 = 34.0;
/// Horizontal padding inside the input row and result rows.
const ROW_PAD_X: f64 = 12.0;
/// Gap between the prompt glyph and the input field.
const PROMPT_GAP: f64 = 8.0;
/// Minimum panel width, in logical px.
const MIN_WIDTH: f64 = 360.0;
/// Maximum panel width, in logical px.
const MAX_WIDTH: f64 = 560.0;
/// Fraction of the available height the panel's top edge sits at (a palette
/// floats near the top, not centered).
const TOP_FRACTION: f64 = 0.14;

/// Prompt marker glyph (a shell-style caret prefix).
const PROMPT: &str = ">";
/// Prompt/label/hint type sizes, in logical px.
const PROMPT_SIZE: f32 = 13.0;
const LABEL_SIZE: f32 = 13.0;
const HINT_SIZE: f32 = 11.0;
const LINE_HEIGHT: f32 = 1.4;

/// Unthemed-fallback panel surface (Glyph dark `bg-overlay` `#272d3d`).
const CONTAINER: Color = Color::from_rgb8(0x27, 0x2d, 0x3d);
/// Unthemed-fallback amber ring (Glyph dark accent `#ffb627`).
const RING: Color = Color::from_rgb8(0xff, 0xb6, 0x27);
/// Unthemed-fallback label ink (Glyph dark `fg` `#f2ead9`).
const LABEL_INK: Color = Color::from_rgb8(0xf2, 0xea, 0xd9);
/// Unthemed-fallback hint/muted ink (Glyph dark `fg-muted` `#a39c88`).
const MUTED_INK: Color = Color::from_rgb8(0xa3, 0x9c, 0x88);
/// Unthemed-fallback selected-row hover wash (Glyph dark `bg-hover`).
const HOVER: Color = Color::from_rgb8(0x22, 0x28, 0x35);
/// Unthemed-fallback scrim base color.
const SCRIM: Color = Color::from_rgb8(0x00, 0x00, 0x00);
/// Unthemed-fallback corner radius (Glyph `--radius-lg` 14px).
const RADIUS: f64 = 14.0;
/// The ring's alpha over its amber accent (a soft glow, not a hard 1px line).
const RING_ALPHA: f32 = 0.7;
/// Ring width, in logical px.
const RING_WIDTH: f64 = 1.5;
/// Corner-rounding tolerance for the ring stroke.
const PATH_TOLERANCE: f64 = 0.1;
/// Scrim opacity at full enter.
const SCRIM_ALPHA: f32 = 0.55;

/// Chrome-level shadow fallback.
const SHADOW_Y: f64 = 12.0;
const SHADOW_BLUR: f64 = 32.0;
const SHADOW_ALPHA: f32 = 0.45;

/// Panel enter scale start (`0.96 → 1.0`).
const ENTER_SCALE_START: f64 = 0.96;
/// Enter duration fallback (`durations.fast` = 150ms).
const ENTER_DURATION: Duration = Duration::from_millis(150);
/// Enter easing fallback (Glyph `spatial`).
const ENTER_CURVE: Curve = Curve::Cubic(0.34, 1.35, 0.64, 1.0);
/// Exit duration fallback (`durations.instant` = 100ms; snappier than the dialog).
const EXIT_DURATION: Duration = Duration::from_millis(100);
/// Exit easing fallback (Glyph `exit`).
const EXIT_CURVE: Curve = Curve::Cubic(0.4, 0.0, 1.0, 1.0);
/// `reduce_motion`'s collapsed crossfade duration.
const REDUCE_MOTION_DURATION: Duration = Duration::from_millis(100);

fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The `(enter, exit)` [`Timing`]s for the current `reduce_motion` state.
fn resolve_timings(theme: Option<&Theme>) -> (Timing, Timing) {
    if theme.map(|t| t.motion.reduce_motion).unwrap_or(false) {
        let t = Timing::Duration(REDUCE_MOTION_DURATION, Curve::Linear);
        return (t, t);
    }
    match theme {
        Some(t) => (
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.fast / 1000.0),
                t.motion.easing.spatial,
            ),
            Timing::Duration(
                Duration::from_secs_f64(t.motion.durations.instant / 1000.0),
                t.motion.easing.exit,
            ),
        ),
        None => (
            Timing::Duration(ENTER_DURATION, ENTER_CURVE),
            Timing::Duration(EXIT_DURATION, EXIT_CURVE),
        ),
    }
}

fn resolve_container(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_highest,
        None => CONTAINER,
    }
}

fn resolve_ring(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().primary,
        None => RING,
    }
}

fn resolve_label(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().on_surface,
        None => LABEL_INK,
    }
}

fn resolve_muted(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().on_surface_variant,
        None => MUTED_INK,
    }
}

fn resolve_hover(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().surface_container_high,
        None => HOVER,
    }
}

fn resolve_scrim(theme: Option<&Theme>) -> Color {
    match theme {
        Some(t) => t.scheme().scrim,
        None => SCRIM,
    }
}

fn resolve_radius(theme: Option<&Theme>) -> f64 {
    match theme {
        Some(t) => t.shape.large,
        None => RADIUS,
    }
}

fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(t) => {
            let s = t.elevation.level5.shadow(t.brightness);
            (
                s.y_offset,
                s.blur_std_dev,
                with_alpha(t.scheme().shadow, s.color_alpha),
            )
        }
        None => (SHADOW_Y, SHADOW_BLUR, with_alpha(SCRIM, SHADOW_ALPHA)),
    }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

fn mono(family: &str, size: f32, weight: FontWeight, color: Color) -> TextStyle {
    TextStyle {
        family: FontFamily::stack_with_generic([family], GenericSlot::Monospace),
        weight,
        line_height: LineHeight::FontSizeRelative(LINE_HEIGHT),
        ..TextStyle::new(size, color)
    }
}

/// One row of the palette: a label plus an optional trailing hint (a kbd
/// shortcut label). See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteItem {
    /// The primary row label.
    pub label: String,
    /// An optional trailing hint (e.g. a keyboard shortcut), right-aligned.
    pub hint: Option<String>,
}

impl PaletteItem {
    /// A row with just a label (no trailing hint).
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
        }
    }

    /// Attach a trailing hint (kbd shortcut) to this row.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// A view-held query-change callback.
type OnQuery<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held row-select callback (carrying the selected index).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;
/// A state-free "close this palette" callback (see [`crate::glyph::dialog`]).
type OnClose = Rc<dyn Fn()>;

/// A declarative Glyph command palette. See the [module docs](self).
pub struct CommandPaletteView<State: 'static> {
    query: String,
    placeholder: String,
    items: Vec<PaletteItem>,
    on_query: OnQuery<State>,
    on_select: OnSelect<State>,
    dismissable: bool,
    on_close: Option<OnClose>,
    /// The shared back-press dismiss-signal cell (see the [module docs](self)'s
    /// Dismissable section). Wired by [`show_command_palette`]; `None` when the
    /// palette is built/tested standalone, outside a navigator push.
    dismiss_signal: Option<Rc<Cell<u64>>>,
}

/// Create a command palette over `items`, streaming query changes to `on_query`
/// and row selections (by index into `items`) to `on_select`. Chain
/// [`query`](CommandPaletteView::query)/[`placeholder`](CommandPaletteView::placeholder)/
/// [`on_close`](CommandPaletteView::on_close); [`show_command_palette`] wires the
/// close for you.
pub fn command_palette<State, Q, S>(
    items: Vec<PaletteItem>,
    on_query: Q,
    on_select: S,
) -> CommandPaletteView<State>
where
    State: 'static,
    Q: Fn(&mut State, String) + 'static,
    S: Fn(&mut State, usize) + 'static,
{
    CommandPaletteView {
        query: String::new(),
        placeholder: "Search…".to_string(),
        items,
        on_query: Rc::new(on_query),
        on_select: Rc::new(on_select),
        dismissable: true,
        on_close: None,
        dismiss_signal: None,
    }
}

impl<State: 'static> CommandPaletteView<State> {
    /// Set the current query text (controlled — the app owns this value).
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }

    /// Set the input placeholder shown while the query is empty.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Whether the palette can be dismissed by a scrim tap, Escape, or a
    /// routed back press (default `true`) — see the [module docs](self)'s
    /// Dismissable section. `false` turns the palette into a modal barrier for
    /// all three; the explicit close paths (`on_close`, a row's `on_select`)
    /// keep working either way. [`show_command_palette`] reads this to choose
    /// the pushed page's [`BackPolicy`].
    pub fn dismissable(mut self, dismissable: bool) -> Self {
        self.dismissable = dismissable;
        self
    }

    /// Set the state-free close callback (see [`crate::glyph::dialog`]). Fired
    /// once from paint when the exit animation completes after a scrim/Escape/
    /// back-dismiss cancel; [`show_command_palette`] wires it to
    /// `controller.pop()`.
    pub fn on_close<F: Fn() + 'static>(mut self, on_close: F) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// Wire the shared back-press dismiss-signal cell (see the
    /// [module docs](self)'s Dismissable section and [`BackPolicy`]'s
    /// observation seam). Internal wiring [`show_command_palette`] performs —
    /// not meant to be called directly by app code.
    pub fn dismiss_signal(mut self, signal: Rc<Cell<u64>>) -> Self {
        self.dismiss_signal = Some(signal);
        self
    }
}

/// Build the palette's [`TextInput`](crate::TextInput) child view — the query
/// value plus an `on_change` adapter that streams into `on_query`.
fn input_view<State: 'static>(
    query: &str,
    placeholder: &str,
    on_query: OnQuery<State>,
) -> AnyView<State> {
    let oq = on_query;
    any::<State, _>(
        text_input(query.to_string(), move |s: &mut State, t: String| oq(s, t))
            .placeholder(placeholder.to_string())
            .text_style(mono(
                "IBM Plex Mono",
                LABEL_SIZE,
                FontWeight::REGULAR,
                LABEL_INK,
            )),
    )
}

/// Push `build`'s palette as a transparent navigator page and register
/// `on_result` for the value it pops with. The scrim/Escape/back cancel is
/// wired to `controller.pop()` (an empty [`PopResult`]); the pushed page's
/// [`BackPolicy`] follows the built palette's
/// [`dismissable`](CommandPaletteView::dismissable) flag — see the
/// [module docs](self)'s Dismissable section.
pub fn show_command_palette<State, B, R>(
    controller: &NavigatorController<State>,
    build: B,
    on_result: R,
) where
    State: 'static,
    B: Fn() -> CommandPaletteView<State> + 'static,
    R: Fn(&mut State, PopResult) + 'static,
{
    let close_ctrl = controller.clone();
    // Peek the configured `dismissable` flag once, up front, to pick the
    // pushed page's back-press policy — a pure read of the (side-effect-free)
    // builder, mirroring `NavigatorController::push_with_options`'s contract
    // that a page's back policy is fixed at push time.
    let dismissable = build().dismissable;
    let back_policy = if dismissable {
        BackPolicy::DismissAnimated
    } else {
        BackPolicy::Veto
    };
    let dismiss_signal = Rc::new(Cell::new(0u64));
    let signal_for_options = dismiss_signal.clone();
    controller.push_with_options(
        move || {
            let ctrl = close_ctrl.clone();
            any::<State, _>(
                build()
                    .on_close(move || ctrl.pop())
                    .dismiss_signal(dismiss_signal.clone()),
            )
        },
        PushOptions::transparent()
            .transition(TransitionSpec::NONE)
            .back(back_policy)
            .dismiss_signal(signal_for_options)
            .on_result(on_result),
    );
}

/// A minimal retained text run (mirrors `crate::glyph::alert`'s `GlyphLabel`).
struct GlyphLabel {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl GlyphLabel {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
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

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for run in layout.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// A shaped result row: its data plus lazily-shaped label/hint runs.
struct Row {
    label: GlyphLabel,
    label_size: Size,
    hint: Option<GlyphLabel>,
    hint_size: Size,
}

/// The palette's enter/exit lifecycle phase (see [`crate::glyph::dialog`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Enter,
    Shown,
    Exit,
    Dismissed,
}

/// The retained widget for a [`CommandPaletteView`]. See the [module docs](self).
pub struct CommandPaletteWidget {
    input: ChildPod,
    prompt: GlyphLabel,
    prompt_size: Size,
    items: Vec<PaletteItem>,
    rows: Vec<Row>,
    on_select: crate::authoring::ErasedArgCallback<usize>,
    dismissable: bool,
    on_close: Option<OnClose>,
    /// The shared back-press dismiss-signal cell (see the [module docs](self)'s
    /// Dismissable section); `None` outside a navigator push.
    dismiss_signal: Option<Rc<Cell<u64>>>,
    /// The last dismiss-signal generation this widget has observed and acted
    /// on (see [`Widget::paint`]'s observation check).
    last_seen_dismiss: u64,
    /// Panel + input-row rects in the widget's own local coordinate space.
    panel: Rect,
    input_rect: Rect,
    /// Each result row's rect (local space), parallel to `rows`/`items`.
    row_rects: Vec<Rect>,
    phase: Phase,
    driver: Option<TransitionDriver>,
    /// Whether the input has been auto-focused (once, on the first event pass).
    autofocused: bool,
    /// The result row a press is in flight on (fires on up-inside).
    pressed_row: Option<usize>,
    /// A scrim (outside-panel) press is in flight.
    scrim_captured: bool,
}

fn make_rows(items: &[PaletteItem]) -> Vec<Row> {
    items
        .iter()
        .map(|it| Row {
            label: GlyphLabel::new(it.label.clone()),
            label_size: Size::ZERO,
            hint: it.hint.as_ref().map(GlyphLabel::new),
            hint_size: Size::ZERO,
        })
        .collect()
}

impl<State: 'static> View<State> for CommandPaletteView<State> {
    type Element = CommandPaletteWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> CommandPaletteWidget {
        let input = crate::authoring::build_child(
            &input_view::<State>(&self.query, &self.placeholder, self.on_query.clone()),
            ctx,
        );
        CommandPaletteWidget {
            input,
            prompt: GlyphLabel::new(PROMPT),
            prompt_size: Size::ZERO,
            items: self.items.clone(),
            rows: make_rows(&self.items),
            on_select: crate::authoring::erase_callback_arg(&self.on_select),
            dismissable: self.dismissable,
            on_close: self.on_close.clone(),
            last_seen_dismiss: self.dismiss_signal.as_ref().map(|s| s.get()).unwrap_or(0),
            dismiss_signal: self.dismiss_signal.clone(),
            panel: Rect::ZERO,
            input_rect: Rect::ZERO,
            row_rects: Vec::new(),
            phase: Phase::Enter,
            driver: None,
            autofocused: false,
            pressed_row: None,
            scrim_captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CommandPaletteWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        let prev_input = input_view::<State>(&prev.query, &prev.placeholder, prev.on_query.clone());
        let next_input = input_view::<State>(&self.query, &self.placeholder, self.on_query.clone());
        flags |= crate::authoring::rebuild_child(&prev_input, &next_input, &mut element.input, ctx);

        if prev.items != self.items {
            element.items = self.items.clone();
            element.rows = make_rows(&self.items);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.dismissable = self.dismissable;
        element.on_select = crate::authoring::erase_callback_arg(&self.on_select);
        element.on_close = self.on_close.clone();
        // A new/changed dismiss-signal identity resets the last-seen generation
        // to its current value, so swapping in a fresh cell never misfires an
        // exit from a stale comparison.
        let signal_changed = self.dismiss_signal.as_ref().map(Rc::as_ptr)
            != element.dismiss_signal.as_ref().map(Rc::as_ptr);
        if signal_changed {
            element.last_seen_dismiss = self.dismiss_signal.as_ref().map(|s| s.get()).unwrap_or(0);
        }
        element.dismiss_signal = self.dismiss_signal.clone();
        flags
    }

    fn teardown(&self, element: &mut CommandPaletteWidget, ctx: &mut BuildCtx<'_>) {
        let input = input_view::<State>(&self.query, &self.placeholder, self.on_query.clone());
        crate::authoring::teardown_child(&input, &mut element.input, ctx);
    }
}

impl CommandPaletteWidget {
    fn begin_exit(&mut self) {
        if matches!(self.phase, Phase::Enter | Phase::Shown) {
            self.phase = Phase::Exit;
            self.driver = None;
        }
    }

    /// Advance the enter/exit phase machine — see [`crate::glyph::dialog`]'s
    /// `advance` (identical shape; the palette's `0.96` scale start and faster
    /// exit are the only differences).
    fn advance(&mut self, now: FrameTime, enter: Timing, exit: Timing) -> (f64, f32, bool) {
        match self.phase {
            Phase::Enter => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(enter).0)
                    .advance(now);
                if adv.done {
                    self.phase = Phase::Shown;
                    self.driver = None;
                }
                let p = adv.value;
                let scale = ENTER_SCALE_START + (1.0 - ENTER_SCALE_START) * p;
                (scale, p.clamp(0.0, 1.0) as f32, !adv.done)
            }
            Phase::Shown => (1.0, 1.0, false),
            Phase::Exit => {
                let adv = self
                    .driver
                    .get_or_insert_with(|| make_driver(exit).0)
                    .advance(now);
                let q = adv.value.clamp(0.0, 1.0);
                let scale = 1.0 - (1.0 - ENTER_SCALE_START) * q;
                if adv.done {
                    self.phase = Phase::Dismissed;
                    self.driver = None;
                    if let Some(on_close) = &self.on_close {
                        on_close();
                    }
                }
                (scale, (1.0 - q) as f32, !adv.done)
            }
            Phase::Dismissed => (ENTER_SCALE_START, 0.0, false),
        }
    }

    /// The result row index containing `pos` (local space), if any.
    fn row_at(&self, pos: Point) -> Option<usize> {
        self.row_rects.iter().position(|r| r.contains(pos))
    }

    /// Whether `event` is the `Down` half of a scrim-dismiss gesture (outside
    /// both the input row and every result row) — see the [module docs](self)'s
    /// focus note. Used to guard the first-event autofocus so an outside tap's
    /// `Down` never flashes the keyboard up only for its `Up` to dismiss it.
    fn is_scrim_down(&self, event: &InputEvent) -> bool {
        let InputEvent::Pointer(p) = event else {
            return false;
        };
        p.phase == PointerPhase::Down
            && !self.input_rect.contains(p.position)
            && self.row_at(p.position).is_none()
    }

    /// Route a synthetic tap into the input to claim focus on the first
    /// (non-scrim-dismiss) event — see the [module docs](self)'s focus note.
    /// Uses a Down+Up through [`crate::authoring::route_event_single`] so the input's
    /// capture is cleanly released and the focus request propagates up the
    /// modal chain.
    fn autofocus_input(&mut self, ctx: &mut EventCtx) {
        // A point just inside the input pod's own bounds (its origin is offset
        // past the prompt glyph), so `route_event_single` actually routes into it.
        let target = Point::new(
            self.input.origin().x + 2.0,
            self.input.origin().y + INPUT_ROW_H / 2.0,
        );
        let down = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: target,
            button: PointerButton::Primary,
        });
        let up = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Up,
            position: target,
            button: PointerButton::Primary,
        });
        crate::authoring::route_event_single(&mut self.input, ctx, &down);
        crate::authoring::route_event_single(&mut self.input, ctx, &up);
    }
}

impl Widget for CommandPaletteWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve owned token colors up front so the immutable theme borrow of
        // `ctx` ends before the mutable text-layout calls below.
        let (ring_c, label_c, muted_c) = {
            let theme = Theme::from_layout_ctx(ctx);
            (
                resolve_ring(theme),
                resolve_label(theme),
                resolve_muted(theme),
            )
        };
        let area_w = finite_or_zero(bc.max().width);
        let area_h = finite_or_zero(bc.max().height);

        let panel_w = area_w.clamp(0.0, MAX_WIDTH).max(MIN_WIDTH.min(area_w));

        // Prompt glyph.
        self.prompt_size = self.prompt.layout(
            ctx,
            &mono("Space Mono", PROMPT_SIZE, FontWeight::BOLD, ring_c),
        );

        // Input field: fills the row minus the prompt + paddings.
        let input_x = ROW_PAD_X + self.prompt_size.width + PROMPT_GAP;
        let input_w = (panel_w - input_x - ROW_PAD_X).max(0.0);
        self.input
            .layout_child(ctx, &BoxConstraints::tight(Size::new(input_w, INPUT_ROW_H)));

        // Result rows.
        let label_style = mono("IBM Plex Mono", LABEL_SIZE, FontWeight::REGULAR, label_c);
        let hint_style = mono("IBM Plex Mono", HINT_SIZE, FontWeight::REGULAR, muted_c);
        for row in &mut self.rows {
            row.label_size = row.label.layout(ctx, &label_style);
            row.hint_size = row
                .hint
                .as_mut()
                .map(|h| h.layout(ctx, &hint_style))
                .unwrap_or(Size::ZERO);
        }

        let rows_h = self.rows.len() as f64 * ROW_H;
        let panel_h = PANEL_PAD * 2.0 + INPUT_ROW_H + rows_h;

        let panel_x = ((area_w - panel_w) / 2.0).max(0.0);
        let panel_y = ((area_h - panel_h) * TOP_FRACTION).max(0.0);
        self.panel = Rect::new(panel_x, panel_y, panel_x + panel_w, panel_y + panel_h);
        self.input_rect = Rect::new(
            panel_x + PANEL_PAD,
            panel_y + PANEL_PAD,
            panel_x + panel_w - PANEL_PAD,
            panel_y + PANEL_PAD + INPUT_ROW_H,
        );
        self.input
            .set_origin(Point::new(self.input_rect.x0 + input_x, self.input_rect.y0));

        self.row_rects.clear();
        let mut ry = self.input_rect.y1;
        for _ in &self.rows {
            self.row_rects.push(Rect::new(
                panel_x + PANEL_PAD,
                ry,
                panel_x + panel_w - PANEL_PAD,
                ry + ROW_H,
            ));
            ry += ROW_H;
        }

        bc.constrain(Size::new(area_w, area_h))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Observe a routed back-press dismiss-signal bump (see the
        // [module docs](self)'s Dismissable section and `BackPolicy`'s
        // observation seam): the navigator flags only `PAINT` dirty on a
        // `DismissAnimated` back request, so this is the one place the widget
        // can notice it and stage the same animated exit a scrim/Escape
        // cancel does.
        if let Some(generation) = self.dismiss_signal.as_ref().map(|s| s.get())
            && generation != self.last_seen_dismiss
        {
            self.last_seen_dismiss = generation;
            self.begin_exit();
        }

        let theme = Theme::from_paint_ctx(ctx);
        let (enter, exit) = resolve_timings(theme);
        let (scale, scrim_frac, animating) = self.advance(ctx.frame_time(), enter, exit);
        let origin_v = ctx.origin().to_vec2();

        // Scrim — its own fade, never scaled with the panel.
        let scrim = with_alpha(resolve_scrim(theme), SCRIM_ALPHA * scrim_frac);
        scene.fill_rect(ctx.origin(), ctx.size(), scrim);

        // Panel scales about its own center.
        let panel_origin = ctx.origin() + self.panel.origin().to_vec2();
        let panel_size = self.panel.size();
        let center = panel_origin + Vec2::new(panel_size.width / 2.0, panel_size.height / 2.0);
        let transform = Affine::translate((center.x, center.y))
            * Affine::scale(scale)
            * Affine::translate((-center.x, -center.y));
        scene.push_transform(transform);

        let radius = resolve_radius(theme);
        let (shadow_y, shadow_blur, shadow_color) = resolve_shadow(theme);
        scene.draw_shadow(
            Point::new(panel_origin.x, panel_origin.y + shadow_y),
            panel_size,
            radius,
            shadow_blur,
            shadow_color,
        );
        scene.fill_rounded_rect(panel_origin, panel_size, radius, resolve_container(theme));

        // Selected-row hover wash (the pressed row), painted under the ring.
        if let Some(i) = self.pressed_row
            && let Some(r) = self.row_rects.get(i)
        {
            scene.fill_rect(
                ctx.origin() + r.origin().to_vec2(),
                r.size(),
                resolve_hover(theme),
            );
        }

        // Amber ring.
        let rr = RoundedRect::from_rect(Rect::from_origin_size(Point::ORIGIN, panel_size), radius);
        let path = rr.to_path(PATH_TOLERANCE);
        scene.stroke_path(
            panel_origin,
            &path,
            RING_WIDTH,
            &Brush::Solid(with_alpha(resolve_ring(theme), RING_ALPHA)),
        );

        // Prompt glyph, vertically centered in the input row.
        let prompt_origin = Point::new(
            self.input_rect.x0 + ROW_PAD_X,
            self.input_rect.y0 + (INPUT_ROW_H - self.prompt_size.height) / 2.0,
        ) + origin_v;
        self.prompt.paint(prompt_origin, scene);

        // The input field.
        self.input.paint_child(ctx, scene);

        // Result rows: label left, hint right.
        for (i, row) in self.rows.iter().enumerate() {
            let Some(r) = self.row_rects.get(i) else {
                continue;
            };
            let label_origin = Point::new(
                r.x0 + ROW_PAD_X,
                r.y0 + (ROW_H - row.label_size.height) / 2.0,
            ) + origin_v;
            row.label.paint(label_origin, scene);
            if let Some(hint) = &row.hint {
                let hint_origin = Point::new(
                    r.x1 - ROW_PAD_X - row.hint_size.width,
                    r.y0 + (ROW_H - row.hint_size.height) / 2.0,
                ) + origin_v;
                hint.paint(hint_origin, scene);
            }
        }

        scene.pop_transform();

        if animating || self.phase == Phase::Dismissed {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // Auto-focus the input on the first event pass — unless that very
        // first event is the `Down` half of a scrim-dismiss gesture (see the
        // module docs' focus note and `is_scrim_down`): claiming focus there
        // would flash the keyboard up only for the matching `Up` to dismiss it.
        if !self.autofocused {
            self.autofocused = true;
            if !self.is_scrim_down(event) {
                self.autofocus_input(ctx);
            }
        }

        // Focus-routed events (Key/Ime) go to the input — except Escape, which
        // cancels the palette when dismissable (a no-op, consumed, otherwise).
        if event.is_focus_routed() {
            if let InputEvent::Key(k) = event
                && k.key == Key::Named(NamedKey::Escape)
            {
                if self.dismissable {
                    self.begin_exit();
                }
                return EventResult::Handled;
            }
            return crate::authoring::route_event_single(&mut self.input, ctx, event);
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };

        // A captured input drag stays with the input.
        if self.input.is_active() {
            return crate::authoring::route_event_single(&mut self.input, ctx, event);
        }

        match p.phase {
            PointerPhase::Down => {
                if self.input_rect.contains(p.position) {
                    return crate::authoring::route_event_single(&mut self.input, ctx, event);
                }
                if let Some(i) = self.row_at(p.position) {
                    self.pressed_row = Some(i);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                // Outside the panel (or the panel background): the modal barrier.
                self.scrim_captured = true;
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.pressed_row.is_some() || self.scrim_captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up => {
                if let Some(i) = self.pressed_row.take() {
                    if self.row_at(p.position) == Some(i) {
                        (self.on_select)(ctx, i);
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if self.scrim_captured {
                    let outside = !self.panel.contains(p.position);
                    if self.dismissable && outside {
                        self.begin_exit();
                    }
                    self.scrim_captured = false;
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            PointerPhase::Cancel => {
                // Cancel never touches state — just clear the in-flight flags.
                self.pressed_row = None;
                self.scrim_captured = false;
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::Dialog,
            |node| {
                node.set_modal();
                node.set_label("Command palette");
            },
            |ctx| {
                self.input.semantics_child(ctx);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::navigator::{NavigatorView, navigator};
    use frust_core::{KeyEvent, Modifiers, RenderRoot, any as core_any};
    use std::any::Any;
    use std::cell::Cell;

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn char_key(c: &str) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character(c.to_string()),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn items(n: usize) -> Vec<PaletteItem> {
        (0..n)
            .map(|i| PaletteItem::new(format!("Item {i}")).hint(format!("⌘{i}")))
            .collect()
    }

    #[derive(Default)]
    struct AppState {
        queries: Vec<String>,
        selected: Vec<usize>,
    }

    fn build_widget(view: &CommandPaletteView<AppState>) -> CommandPaletteWidget {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CommandPaletteWidget) {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(800.0, 600.0)));
    }

    fn dispatch(
        w: &mut CommandPaletteWidget,
        state: &mut AppState,
        event: &InputEvent,
    ) -> EventResult {
        let s: &mut dyn Any = state;
        let mut ctx = EventCtx::new(s, Point::ZERO, Size::new(800.0, 600.0));
        w.event(&mut ctx, event)
    }

    // -- Focus lands in the input on open (first event) -----------------

    #[test]
    fn first_event_claims_focus_for_the_input() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(3),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        layout(&mut w);
        assert!(!w.input.is_focused(), "not yet focused before any event");

        let mut state = AppState::default();
        // Any first event triggers the autofocus routing.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 5.0, 5.0));
        assert!(w.autofocused);
        assert!(
            w.input.is_focused(),
            "the input claims focus on the first event"
        );
    }

    #[test]
    fn scrim_down_as_the_first_event_never_claims_focus_and_still_dismisses() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(3),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        layout(&mut w);
        w.phase = Phase::Shown;
        assert!(!w.input.is_focused());

        let mut state = AppState::default();
        // A scrim tap (well outside the panel) delivered as the palette's very
        // first event: Down must not autofocus (no keyboard flash), and the
        // matching Up still begins the exit (the dismiss itself still works).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 2.0, 2.0));
        assert!(w.autofocused, "the first-event gate still trips once");
        assert!(
            !w.input.is_focused(),
            "a scrim Down never claims focus, even as the first event"
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 2.0, 2.0));
        assert!(
            !w.input.is_focused(),
            "no focus claim occurs anywhere in the scrim Down+Up sequence"
        );
        assert_eq!(w.phase, Phase::Exit, "the scrim tap still begins the exit");
    }

    #[test]
    fn typing_streams_on_query_after_focus() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(2),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();

        // First event auto-focuses; a Key then flows into the field via on_query.
        dispatch(&mut w, &mut state, &char_key("a"));
        assert!(w.input.is_focused());
        assert_eq!(
            state.queries,
            vec!["a".to_string()],
            "on_query streams the typed char"
        );
    }

    // -- on_select fires on row up-inside -------------------------------

    #[test]
    fn row_up_inside_fires_on_select() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(3),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();

        // Press+release inside row 1.
        let r = w.row_rects[1];
        let (cx, cy) = (r.center().x, r.center().y);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, cx, cy));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, cx, cy));
        assert_eq!(state.selected, vec![1], "up-inside a row selects it");
    }

    #[test]
    fn release_outside_the_pressed_row_does_not_select() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(3),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();

        let r1 = w.row_rects[1];
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, r1.center().x, r1.center().y),
        );
        // Release over a different row.
        let r2 = w.row_rects[2];
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, r2.center().x, r2.center().y),
        );
        assert!(
            state.selected.is_empty(),
            "a release off the pressed row cancels"
        );
    }

    // -- Enter/exit staging: exit faster than enter ---------------------

    #[test]
    fn exit_is_faster_than_enter() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(1),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        let (enter, exit) = resolve_timings(None);

        // Enter: 150ms. Seed then settle.
        w.advance(ft_ms(0.0), enter, exit);
        let (_, _, animating_enter_mid) = w.advance(ft_ms(120.0), enter, exit);
        assert!(
            animating_enter_mid,
            "still entering at 120ms (< 150ms enter)"
        );
        w.advance(ft_ms(200.0), enter, exit);
        assert_eq!(w.phase, Phase::Shown);

        // Exit: 100ms — done before the 150ms an enter would take.
        w.begin_exit();
        w.advance(ft_ms(200.0), enter, exit); // seed exit clock
        let (_, _, animating_exit_mid) = w.advance(ft_ms(200.0 + 99.0), enter, exit);
        assert!(animating_exit_mid);
        let (_, _, animating_exit_end) = w.advance(ft_ms(200.0 + 150.0), enter, exit);
        assert!(!animating_exit_end, "the 100ms exit is done by 150ms");
        assert_eq!(w.phase, Phase::Dismissed);
    }

    #[test]
    fn scrim_cancel_begins_exit_and_fires_on_close() {
        let closed = Rc::new(Cell::new(0u32));
        let c = closed.clone();
        let view: CommandPaletteView<AppState> = command_palette(
            items(1),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        )
        .on_close(move || c.set(c.get() + 1));
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();
        w.phase = Phase::Shown;

        // Press+release outside the panel.
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 2.0, 2.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 2.0, 2.0));
        assert_eq!(w.phase, Phase::Exit);
        let (enter, exit) = resolve_timings(None);
        w.advance(ft_ms(0.0), enter, exit);
        assert_eq!(closed.get(), 0, "on_close waits for the exit to finish");
        w.advance(ft_ms(300.0), enter, exit);
        assert_eq!(closed.get(), 1);
    }

    // -- dismissable(false): scrim + Escape are no-ops, close still works ---

    #[test]
    fn dismissable_false_blocks_scrim_tap() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(1),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        )
        .dismissable(false);
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();
        w.phase = Phase::Shown;

        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 2.0, 2.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 2.0, 2.0));
        assert_eq!(
            w.phase,
            Phase::Shown,
            "a scrim tap is a no-op when not dismissable"
        );
    }

    #[test]
    fn dismissable_false_blocks_escape() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(1),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        )
        .dismissable(false);
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();
        w.phase = Phase::Shown;

        let escape = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Modifiers::default(),
            repeat: false,
        });
        let result = dispatch(&mut w, &mut state, &escape);
        assert_eq!(
            w.phase,
            Phase::Shown,
            "Escape is a no-op when not dismissable"
        );
        assert_eq!(result, EventResult::Handled, "Escape is still consumed");
    }

    #[test]
    fn dismissable_false_still_allows_on_select_close() {
        let view: CommandPaletteView<AppState> = command_palette(
            items(3),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        )
        .dismissable(false);
        let mut w = build_widget(&view);
        layout(&mut w);
        let mut state = AppState::default();

        let r = w.row_rects[0];
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, r.center().x, r.center().y),
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, r.center().x, r.center().y),
        );
        assert_eq!(
            state.selected,
            vec![0],
            "the explicit row-select close path always works"
        );
    }

    // -- Back-press parity: dismiss-signal observation -----------------

    #[test]
    fn dismiss_signal_bump_begins_exit_on_next_paint() {
        let signal = Rc::new(Cell::new(0u64));
        let view: CommandPaletteView<AppState> = command_palette(
            items(1),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        )
        .dismiss_signal(signal.clone());
        let mut w = build_widget(&view);
        layout(&mut w);
        w.phase = Phase::Shown;

        let mut rec = Recorder::default();
        let area = Size::new(800.0, 600.0);
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(w.phase, Phase::Shown, "no bump yet — nothing happens");

        // A routed back press bumps the shared generation cell.
        signal.set(signal.get() + 1);
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(
            w.phase,
            Phase::Exit,
            "the widget observes the bump on its next paint and begins exit"
        );

        // A second paint with no further bump must not re-trigger.
        w.phase = Phase::Shown;
        let mut pctx = PaintCtx::new(Point::ZERO, area);
        w.paint(&mut pctx, &mut rec);
        assert_eq!(
            w.phase,
            Phase::Shown,
            "an already-observed generation never re-triggers"
        );
    }

    // -- Recording-scene token assertions (dark + light) ----------------

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<Color>,
        transforms: usize,
        transform_pops: u32,
    }
    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn draw_shadow(&mut self, _o: Point, _s: Size, _r: f64, _b: f64, _c: Color) {}
        fn stroke_path(&mut self, _o: Point, _p: &kurbo::BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(c) = brush {
                self.strokes.push(*c);
            }
        }
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {}
        fn push_transform(&mut self, _t: Affine) {
            self.transforms += 1;
        }
        fn pop_transform(&mut self) {
            self.transform_pops += 1;
        }
    }

    fn paint_shown(theme: &Theme) -> Recorder {
        let view: CommandPaletteView<AppState> = command_palette(
            items(2),
            |s: &mut AppState, q| s.queries.push(q),
            |s: &mut AppState, i: usize| s.selected.push(i),
        );
        let mut w = build_widget(&view);
        let mut tcx = TextContext::new();
        let area = Size::new(800.0, 600.0);
        let mut lctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), Some(theme as &dyn Any));
        w.layout(&mut lctx, &BoxConstraints::tight(area));
        w.phase = Phase::Shown;
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, area).with_theme(theme);
        w.paint(&mut pctx, &mut rec);
        rec
    }

    #[test]
    fn panel_ring_and_scrim_resolve_dark_and_light() {
        let dark = Theme::glyph_baseline();
        let light = dark.clone().with_brightness(frust_theme::Brightness::Light);
        for theme in [&dark, &light] {
            let rec = paint_shown(theme);
            // Scrim: the first full-area fill, at full opacity when Shown.
            assert_eq!(rec.rects[0].1, Size::new(800.0, 600.0));
            assert_eq!(
                rec.rects[0].2,
                with_alpha(theme.scheme().scrim, SCRIM_ALPHA)
            );
            // Panel surface fill.
            assert_eq!(rec.rrects[0].3, theme.scheme().surface_container_highest);
            assert_eq!(rec.rrects[0].2, theme.shape.large);
            // Amber ring stroke.
            assert_eq!(
                rec.strokes[0],
                with_alpha(theme.scheme().primary, RING_ALPHA)
            );
            assert_eq!(rec.transforms, 1);
            assert_eq!(rec.transform_pops, 1);
        }
        // Dark and light panels differ.
        assert_ne!(
            Theme::glyph_baseline().scheme().surface_container_highest,
            Theme::glyph_baseline()
                .with_brightness(frust_theme::Brightness::Light)
                .scheme()
                .surface_container_highest
        );
    }

    // -- Navigator integration: scrim cancel pops with an empty result --

    #[derive(Default)]
    struct NavState {
        results: Vec<Option<usize>>,
    }

    fn app_page(
        controller: &NavigatorController<NavState>,
    ) -> impl FnMut(&mut NavState) -> NavigatorView<NavState> {
        let ctrl = controller.clone();
        move |_: &mut NavState| navigator(&ctrl, || core_any::<NavState, _>(sized(800.0, 600.0)))
    }

    struct Sized {
        size: Size,
    }
    struct SizedW {
        size: Size,
    }
    impl View<NavState> for Sized {
        type Element = SizedW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> SizedW {
            SizedW { size: self.size }
        }
        fn rebuild(&self, _p: &Self, _e: &mut SizedW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for SizedW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(self.size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            scene.fill_rect(ctx.origin(), ctx.size(), Color::BLACK);
        }
    }
    fn sized(w: f64, h: f64) -> Sized {
        Sized {
            size: Size::new(w, h),
        }
    }

    #[test]
    fn show_command_palette_dismisses_and_clears_focus_via_navigator() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(800.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_command_palette(
            &controller,
            || command_palette(items(3), |_s: &mut NavState, _q| {}, |_s, _i| {}),
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<usize>()),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(300.0));

        // A tap on the input row is the palette's first event: it auto-focuses
        // the field and the shell tracks focus through the whole modal chain.
        root.event(&mut state, &ev(PointerPhase::Down, 300.0, 90.0));
        root.event(&mut state, &ev(PointerPhase::Up, 300.0, 90.0));
        assert!(
            root.is_focus_active(),
            "the input claimed focus through the modal chain"
        );

        // Scrim cancel: press+release near the bottom, well outside the top panel.
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 590.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 590.0));
        root.paint(&mut Recorder::default(), ft_ms(400.0));
        root.paint(&mut Recorder::default(), ft_ms(700.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "scrim cancel pops with an empty result"
        );
        // The palette page is gone; its focus/IME went with it (suppressing
        // container). No capture dangles into the revealed page.
        assert!(
            !root.is_pointer_captured(),
            "no capture survives the dismiss"
        );
    }

    #[test]
    fn back_request_animates_exit_then_pops_when_dismissable() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(800.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_command_palette(
            &controller,
            || command_palette(items(3), |_s: &mut NavState, _q| {}, |_s, _i| {}),
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<usize>()),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(300.0));
        assert_eq!(controller.depth(), 2);

        // Route a back press (the `BackPolicy` entry point) instead of a scrim tap.
        controller.request_back();
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        assert_eq!(
            controller.depth(),
            2,
            "DismissAnimated leaves the stack unchanged immediately"
        );

        // The widget observes the dismiss-signal bump on its next paint and
        // stages an animated exit, popping itself once it completes.
        root.paint(&mut Recorder::default(), ft_ms(400.0));
        root.paint(&mut Recorder::default(), ft_ms(700.0));
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.event(&mut state, &ev(PointerPhase::Move, 5.0, 5.0));

        assert_eq!(
            state.results,
            vec![None],
            "back-dismiss pops with an empty result"
        );
        assert_eq!(controller.depth(), 1, "the palette page is gone");
    }

    #[test]
    fn back_request_is_vetoed_when_not_dismissable() {
        let controller: NavigatorController<NavState> = NavigatorController::new();
        let mut root: RenderRoot<NavState, NavigatorView<NavState>> = RenderRoot::new();
        let mut app = app_page(&controller);
        let mut state = NavState::default();
        let area = Size::new(800.0, 600.0);

        root.rebuild(&mut app, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(area, &mut tcx as &mut dyn Any);

        show_command_palette(
            &controller,
            || {
                command_palette(items(3), |_s: &mut NavState, _q| {}, |_s, _i| {})
                    .dismissable(false)
            },
            |state: &mut NavState, result: PopResult| state.results.push(result.take::<usize>()),
        );
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(0.0));
        root.paint(&mut Recorder::default(), ft_ms(300.0));
        assert_eq!(controller.depth(), 2);

        controller.request_back();
        root.rebuild(&mut app, &mut state);
        root.layout_with_text(area, &mut tcx as &mut dyn Any);
        root.paint(&mut Recorder::default(), ft_ms(400.0));

        assert_eq!(
            controller.depth(),
            2,
            "Veto leaves the stack unchanged — the back press is consumed but nothing happens"
        );
        assert!(
            state.results.is_empty(),
            "no pop, so no result callback fires"
        );
    }

    #[test]
    fn semantics_is_a_modal_dialog() {
        fn logic(_s: &mut AppState) -> CommandPaletteView<AppState> {
            command_palette(
                items(1),
                |s: &mut AppState, q| s.queries.push(q),
                |s: &mut AppState, i: usize| s.selected.push(i),
            )
        }
        let mut root: RenderRoot<AppState, CommandPaletteView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(800.0, 600.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Dialog)
            .expect("a Role::Dialog node is contributed");
        assert!(node.is_modal());
        assert_eq!(node.label(), Some("Command palette"));
    }
}
