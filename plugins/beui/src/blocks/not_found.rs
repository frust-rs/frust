//! Ports beUI's `not-found` block — `components/motion/not-found/*` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `not-found`: *"Animated 404 pages in five styles: glitch scramble,
//! magnetic digits, cursor spotlight, a fanning card stack and a typed
//! terminal."*
//!
//! Upstream ships six files behind one slug: a `shared.tsx` (the stage, the
//! defaults and the dual call-to-action) and one file per style. All six are
//! carried here, as [`NotFoundStyle`]'s five arms over one widget — a single
//! retained element is what lets a caller switch styles without the stage,
//! the copy and the actions being rebuilt around it.
//!
//! | upstream | here |
//! |---|---|
//! | `NOT_FOUND_DEFAULTS` (`404`, the title, the description, both labels) | [`NOT_FOUND_CODE`] and the [`not_found`] builder's defaults |
//! | `NotFoundStage` `min-h-[420px] gap-8` | [`NOT_FOUND_STAGE_HEIGHT`], [`NOT_FOUND_STAGE_GAP`] |
//! | `NotFoundActions`' `h-11 rounded-full` pair on `SPRING_PRESS` | [`NOT_FOUND_ACTION_HEIGHT`], [`NotFoundAction`] |
//! | `glitch.tsx`'s `GLYPHS`/`SCRAMBLE_MS = 700`/`TICK_MS = 45` | [`GLITCH_GLYPHS`], [`GLITCH_SCRAMBLE`], [`GLITCH_TICK`], [`glitch_settled`] |
//! | its `#ff0040` / `#00e5ff` chromatic ghosts, `±3px` on hover | [`GLITCH_GHOST_WARM`], [`GLITCH_GHOST_COOL`], [`GLITCH_GHOST_SHIFT`] |
//! | `magnetic.tsx`'s `Magnetic strength={0.6}` | [`MAGNETIC_STRENGTH`], over [`crate::motion::PointerTracker`] |
//! | `spotlight.tsx`'s `radial-gradient(220px circle …)` over `text-white/10` | [`SPOTLIGHT_RADIUS`], [`SPOTLIGHT_DIM_ALPHA`] |
//! | `stacked.tsx`'s `h-44 w-64` deck and its two `rotate ∓9 / x ∓28 / y 8` cards | [`stacked_card`], [`STACK_LIFT`] |
//! | `terminal.tsx`'s three `TextReveal` lines and its blinking caret | [`TERMINAL_LINES`], [`TERMINAL_CHAR_STAGGER`], [`TERMINAL_CARET_PERIOD`] |
//!
//! # The scramble is deterministic, on purpose
//!
//! Upstream's glitch picks each unsettled character with `Math.random()` on a
//! 45ms tick. A widget here has a frame clock and no RNG — and reaching for one
//! would make the component untestable and its frames unreproducible — so the
//! glyph is chosen by [`glitch_glyph`], a small integer hash of the character's
//! index and the tick number. The result is the same visual (a character
//! churning through unrelated glyphs until its turn to settle) with the same
//! settle schedule, and it is a pure function a test can pin.
//!
//! # Degradations against the web original
//!
//! - **No blur anywhere.** The terminal's `blur(6)` reveal and the glitch's
//!   `mix-blend-screen` ghosts are carried as opacity and offset alone;
//!   `PaintScene` has neither a blur nor a blend-mode primitive.
//! - **The spotlight is a hard-edged circle, not a gradient mask.** Upstream
//!   feathers its reveal from `25%` to `72%` of a 220px radial gradient; the
//!   nearest primitive here is a rounded clip, so the bright code is revealed
//!   inside a [`SPOTLIGHT_RADIUS`] circle with a crisp edge.
//! - **The actions are callbacks, not links.** `homeHref`/`browseHref` are
//!   anchors; a framework has no navigator seam at this tier, so the pair fire
//!   [`NotFoundView::on_action`] with the [`NotFoundAction`] that was pressed.
//! - **No `clamp()` type scale.** The code renders at one size
//!   ([`NOT_FOUND_CODE_SIZE`], or the terminal's own mono size) rather than
//!   tracking the viewport width.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BoxConstraints, BuildCtx, ChangeFlags, Color, ErasedArgCallback, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect, Role,
    SemanticsCtx, Size, TickClass, Vec2, View, Widget, erase_callback_arg, text::TextStyle,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{paint_panel_hairline, resolve_panel};
use crate::motion::{PointerTracker, Ramp};
use crate::press::{Lane, inside, is_activation_key, press_scale, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL, SPRING_PRESS};

// ---- Defaults and metrics --------------------------------------------------

/// `NOT_FOUND_DEFAULTS.code` — the status the page shows.
pub const NOT_FOUND_CODE: &str = "404";

/// `min-h-[420px]` — the stage's least height, in logical px.
pub const NOT_FOUND_STAGE_HEIGHT: f64 = 420.0;

/// `gap-8` — the gap between the stage's three bands, in logical px.
pub const NOT_FOUND_STAGE_GAP: f64 = 32.0;

/// `gap-2` — the gap between the title and the description, in logical px.
pub const NOT_FOUND_COPY_GAP: f64 = 8.0;

/// `gap-3` — the gap between the two actions, in logical px.
pub const NOT_FOUND_ACTION_GAP: f64 = 12.0;

/// `h-11` — an action's height, in logical px.
pub const NOT_FOUND_ACTION_HEIGHT: f64 = 44.0;

/// `px-6` — an action's horizontal padding, in logical px.
pub const NOT_FOUND_ACTION_PADDING_X: f64 = 24.0;

/// The size the status code is drawn at, in logical px — upstream's
/// `clamp(5rem, 18vw, 11rem)` resolved to one value (see the [module docs](self)).
pub const NOT_FOUND_CODE_SIZE: f64 = 96.0;

/// `text-lg font-semibold` — the title's size, in logical px.
pub const NOT_FOUND_TITLE_SIZE: f64 = 18.0;

/// `whileTap={{ scale: 0.96 }}` — an action's press shrink.
pub const NOT_FOUND_PRESS_SCALE: f64 = 0.96;

/// `whileHover={{ scale: 1.02 }}` — an action's hover swell.
pub const NOT_FOUND_HOVER_SCALE: f64 = 1.02;

// ---- The five styles -------------------------------------------------------

/// Which of upstream's five 404 pages this block renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotFoundStyle {
    /// `glitch.tsx`: the digits scramble through random glyphs before
    /// resolving, with a chromatic split on hover.
    #[default]
    Glitch,
    /// `magnetic.tsx`: each digit is cursor-attracted and springs back on
    /// leave.
    Magnetic,
    /// `spotlight.tsx`: a dark panel where a cursor-tracked spotlight reveals
    /// the bright code from a dim base.
    Spotlight,
    /// `stacked.tsx`: a code card over a hidden stack that fans on hover.
    Stacked,
    /// `terminal.tsx`: a terminal window typing a failed `cd` and a 404 status,
    /// with a blinking caret.
    Terminal,
}

impl NotFoundStyle {
    /// All five, in the order upstream's registry entry lists its examples.
    pub const ALL: [NotFoundStyle; 5] = [
        NotFoundStyle::Glitch,
        NotFoundStyle::Magnetic,
        NotFoundStyle::Spotlight,
        NotFoundStyle::Stacked,
        NotFoundStyle::Terminal,
    ];

    /// Whether this style paints its own dark panel rather than sitting on the
    /// page's own background — upstream's `bg-neutral-950` pair.
    pub const fn is_panelled(self) -> bool {
        matches!(self, NotFoundStyle::Spotlight | NotFoundStyle::Terminal)
    }

    /// Whether this style tracks the pointer inside its stage.
    pub const fn tracks_pointer(self) -> bool {
        matches!(self, NotFoundStyle::Magnetic | NotFoundStyle::Spotlight)
    }

    /// The ramp this style's hover effect travels on: the deck fans on
    /// `SPRING_PANEL`, and the glitch's ghosts split on its own short ease.
    /// Every other style has no hover effect and takes the cheaper ramp.
    pub const fn hover_ramp(self) -> Ramp {
        match self {
            NotFoundStyle::Stacked => Ramp::spring(SPRING_PANEL),
            _ => Ramp::eased(GLITCH_GHOST_RAMP, EASE_OUT),
        }
    }
}

/// Which of the two calls to action was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotFoundAction {
    /// `homeLabel` — the primary action (`Back home`).
    Home,
    /// `browseLabel` — the secondary action (`Browse components`).
    Browse,
}

// ---- Glitch ----------------------------------------------------------------

/// `GLYPHS` — the alphabet an unsettled character churns through.
pub const GLITCH_GLYPHS: &str = "ABCDEFGHJKLMNPQRSTUVWXYZ0123456789#%&@$?/\\";

/// `SCRAMBLE_MS = 700` — how long the whole scramble takes.
pub const GLITCH_SCRAMBLE: Duration = Duration::from_millis(700);

/// `TICK_MS = 45` — how often an unsettled character picks a new glyph.
pub const GLITCH_TICK: Duration = Duration::from_millis(45);

/// `text-[#ff0040]` — the warm chromatic ghost.
pub const GLITCH_GHOST_WARM: Color = Color::from_rgb8(0xff, 0x00, 0x40);

/// `text-[#00e5ff]` — the cool one.
pub const GLITCH_GHOST_COOL: Color = Color::from_rgb8(0x00, 0xe5, 0xff);

/// `group-hover:translate-x-[3px]` — how far the ghosts split on hover, in
/// logical px (the cool one goes the other way).
pub const GLITCH_GHOST_SHIFT: f64 = 3.0;

/// `group-hover:opacity-70` — the ghosts' opacity at full hover.
pub const GLITCH_GHOST_ALPHA: f32 = 0.70;

/// `duration-150 ease-out` — how long the ghost split takes.
pub const GLITCH_GHOST_RAMP: Duration = Duration::from_millis(150);

/// How many characters of a `len`-character code have settled `elapsed` into
/// the scramble — `Math.floor(progress * chars.length)`, saturating at `len`.
///
/// Characters settle left to right, so the code resolves in reading order; past
/// [`GLITCH_SCRAMBLE`] every character is its own.
pub fn glitch_settled(elapsed: Duration, len: usize) -> usize {
    if GLITCH_SCRAMBLE.is_zero() || elapsed >= GLITCH_SCRAMBLE {
        return len;
    }
    let progress = elapsed.as_secs_f64() / GLITCH_SCRAMBLE.as_secs_f64();
    ((progress * len as f64).floor() as usize).min(len)
}

/// Which tick of the scramble `elapsed` falls in — the second half of
/// [`glitch_glyph`]'s seed, so a glyph holds for a whole [`GLITCH_TICK`] rather
/// than churning every frame.
pub fn glitch_tick(elapsed: Duration) -> u64 {
    if GLITCH_TICK.is_zero() {
        return 0;
    }
    (elapsed.as_nanos() / GLITCH_TICK.as_nanos()) as u64
}

/// The glyph character `index` shows on `tick` — a deterministic stand-in for
/// upstream's `Math.random()`, for the reason the [module docs](self) give.
///
/// The mixing is one round of a 64-bit integer hash (xorshift-multiply), which
/// is enough to make consecutive ticks and neighbouring indices land on
/// unrelated glyphs — the only property the effect needs.
pub fn glitch_glyph(index: usize, tick: u64) -> char {
    let glyphs: Vec<char> = GLITCH_GLYPHS.chars().collect();
    if glyphs.is_empty() {
        return ' ';
    }
    let mut seed = (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ tick.wrapping_mul(0x1234_5678_9abc_def1);
    seed ^= seed >> 33;
    seed = seed.wrapping_mul(0xff51_afd7_ed55_8ccd);
    seed ^= seed >> 29;
    glyphs[(seed % glyphs.len() as u64) as usize]
}

// ---- Magnetic --------------------------------------------------------------

/// `Magnetic strength={0.6}` — how hard a digit is pulled toward the cursor.
pub const MAGNETIC_STRENGTH: f64 = 0.6;

/// `-ml-2` — how much consecutive digits overlap, in logical px.
pub const MAGNETIC_OVERLAP: f64 = 8.0;

// ---- Spotlight -------------------------------------------------------------

/// `radial-gradient(220px circle …)` — the spotlight's radius, in logical px.
pub const SPOTLIGHT_RADIUS: f64 = 220.0;

/// `text-white/10` — the dim base layer's alpha.
pub const SPOTLIGHT_DIM_ALPHA: f32 = 0.10;

/// `aspect-[16/9]` — the spotlight panel's aspect ratio.
pub const SPOTLIGHT_ASPECT: f64 = 16.0 / 9.0;

/// `max-w-xl` — the spotlight panel's width cap, in logical px.
pub const SPOTLIGHT_MAX_WIDTH: f64 = 576.0;

/// `bg-neutral-950` — the panel both dark styles sit on.
pub const NOT_FOUND_PANEL_INK: Color = Color::from_rgb8(0x0a, 0x0a, 0x0a);

// ---- Stacked ---------------------------------------------------------------

/// `w-64` — the card deck's width, in logical px.
pub const STACK_WIDTH: f64 = 256.0;

/// `h-44` — its height, in logical px.
pub const STACK_HEIGHT: f64 = 176.0;

/// `rounded-3xl` — a deck card's corner radius.
pub const STACK_RADIUS: f64 = style::RADIUS_3XL;

/// The two back cards' rotation at full fan, in degrees — the first turns
/// anticlockwise, the second clockwise.
pub const STACK_ROTATION: f64 = 9.0;

/// Their horizontal spread at full fan, in logical px.
pub const STACK_SPREAD_X: f64 = 28.0;

/// Their downward slip at full fan, in logical px.
pub const STACK_SPREAD_Y: f64 = 8.0;

/// `hover: { y: -6 }` — how far the front card lifts, in logical px.
pub const STACK_LIFT: f64 = 6.0;

/// One deck card's placement at fan progress `t`: its offset from the deck's
/// own box and its rotation in degrees.
///
/// `slot` is `0` and `1` for the two hidden cards and `2` for the front one, in
/// upstream's own paint order. The two back cards mirror each other, so the
/// deck opens symmetrically; the front card only lifts.
///
/// A slot past the deck reports the front card's rule, which is the inert one.
pub fn stacked_card(slot: usize, t: f64) -> (Vec2, f64) {
    let t = t.clamp(0.0, 1.0);
    match slot {
        0 => (
            Vec2::new(-STACK_SPREAD_X * t, STACK_SPREAD_Y * t),
            -STACK_ROTATION * t,
        ),
        1 => (
            Vec2::new(STACK_SPREAD_X * t, STACK_SPREAD_Y * t),
            STACK_ROTATION * t,
        ),
        _ => (Vec2::new(0.0, -STACK_LIFT * t), 0.0),
    }
}

// ---- Terminal --------------------------------------------------------------

/// The three lines the terminal types, with the per-line start delay upstream
/// gives each one.
///
/// The third carries a `{}` placeholder the code is substituted into, which is
/// upstream's `$ status ${code}`.
pub const TERMINAL_LINES: [(&str, Duration); 3] = [
    ("$ cd /page", Duration::ZERO),
    (
        "cd: no such file or directory: /page",
        Duration::from_millis(450),
    ),
    ("$ status {}", Duration::from_millis(1_100)),
];

/// `stagger={0.018}` — the gap between typed characters on the first and third
/// lines.
pub const TERMINAL_CHAR_STAGGER: Duration = Duration::from_millis(18);

/// `stagger={0.012}` — the (faster) gap on the error line.
pub const TERMINAL_ERROR_STAGGER: Duration = Duration::from_millis(12);

/// How long the caret spends on, then off — `animate-pulse`'s own 2s cycle.
pub const TERMINAL_CARET_PERIOD: Duration = Duration::from_millis(2_000);

/// `max-w-md` — the terminal window's width cap, in logical px.
pub const TERMINAL_MAX_WIDTH: f64 = 448.0;

/// The title bar's height, in logical px (`px-4 py-3` around three 12px dots).
pub const TERMINAL_BAR_HEIGHT: f64 = 38.0;

/// `leading-relaxed` on `text-sm` — one typed line's height, in logical px.
pub const TERMINAL_LINE_HEIGHT: f64 = 22.0;

/// The three traffic-light dots, in their own order.
pub const TERMINAL_DOTS: [Color; 3] = [
    Color::from_rgb8(0xff, 0x5f, 0x57),
    Color::from_rgb8(0xfe, 0xbc, 0x2e),
    Color::from_rgb8(0x28, 0xc8, 0x40),
];

/// How many characters of a `len`-character line have been typed at `elapsed`,
/// given the line's own `delay` and per-character `stagger`.
///
/// Nothing is typed before the line's delay opens; from there one character
/// lands every `stagger`, saturating at `len`.
pub fn terminal_typed(elapsed: Duration, delay: Duration, stagger: Duration, len: usize) -> usize {
    let Some(after) = elapsed.checked_sub(delay) else {
        return 0;
    };
    if stagger.is_zero() {
        return len;
    }
    ((after.as_nanos() / stagger.as_nanos()) as usize).min(len)
}

/// Whether the caret is lit at `elapsed` — on for the first half of each
/// [`TERMINAL_CARET_PERIOD`], off for the second.
pub fn terminal_caret_lit(elapsed: Duration) -> bool {
    if TERMINAL_CARET_PERIOD.is_zero() {
        return true;
    }
    let phase = elapsed.as_nanos() % TERMINAL_CARET_PERIOD.as_nanos();
    phase * 2 < TERMINAL_CARET_PERIOD.as_nanos()
}

// ---- The component ---------------------------------------------------------

/// A view-held action callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State, NotFoundAction)>;

/// What the page renders from.
#[derive(Clone, Debug, PartialEq)]
struct PageConfig {
    style: NotFoundStyle,
    code: String,
    title: String,
    description: String,
    home_label: String,
    browse_label: String,
}

/// A declarative beUI 404 page. See [`not_found`].
pub struct NotFoundView<State: 'static> {
    config: PageConfig,
    on_action: OnAction<State>,
}

/// Build a 404 page in [`NotFoundStyle::Glitch`], carrying upstream's own
/// default copy.
pub fn not_found<State: 'static>() -> NotFoundView<State> {
    NotFoundView {
        config: PageConfig {
            style: NotFoundStyle::default(),
            code: NOT_FOUND_CODE.to_string(),
            title: "Page not found".to_string(),
            description: "The page you are looking for moved, vanished, or never existed."
                .to_string(),
            home_label: "Back home".to_string(),
            browse_label: "Browse components".to_string(),
        },
        on_action: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> NotFoundView<State> {
    /// Pick the style (default [`NotFoundStyle::Glitch`]).
    pub fn style(mut self, style: NotFoundStyle) -> Self {
        self.config.style = style;
        self
    }

    /// Set the status code (`code`).
    pub fn code(mut self, code: impl Into<String>) -> Self {
        self.config.code = code.into();
        self
    }

    /// Set the headline (`title`).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    /// Set the body copy (`description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.config.description = description.into();
        self
    }

    /// Set the primary action's label (`homeLabel`).
    pub fn home_label(mut self, label: impl Into<String>) -> Self {
        self.config.home_label = label.into();
        self
    }

    /// Set the secondary action's label (`browseLabel`).
    pub fn browse_label(mut self, label: impl Into<String>) -> Self {
        self.config.browse_label = label.into();
        self
    }

    /// Set the action callback, fired from the pressed action's own release.
    pub fn on_action<F: Fn(&mut State, NotFoundAction) + 'static>(mut self, on_action: F) -> Self {
        self.on_action = Rc::new(on_action);
        self
    }
}

/// The retained widget for a [`NotFoundView`].
pub struct NotFoundWidget {
    config: PageConfig,
    /// One shaped run per grapheme of the code — the cell grid every character
    /// effect (the scramble, the magnetic pull) drives.
    cells: Vec<LabelRun>,
    /// The scramble's substituted glyphs, one per cell, as of the last paint.
    scrambled: Vec<LabelRun>,
    title: LabelRun,
    description: LabelRun,
    home: LabelRun,
    browse: LabelRun,
    /// The terminal's three lines, already substituted.
    lines: Vec<LabelRun>,
    /// The prefix of each line typed so far, one run per line.
    typed: Vec<LabelRun>,
    /// The pointer, for the magnetic pull and the spotlight.
    tracker: PointerTracker,
    /// The glitch ghosts' split, and the stack's fan — both hover-driven.
    hover: Lane,
    /// Each action's press shrink, in [`NotFoundAction`] order.
    press: [Lane; 2],
    /// The frame the style's own run started on, latched at first paint.
    started: Option<FrameTime>,
    /// The elapsed time as of the last paint — what `layout` shapes the
    /// Glitch scramble's and the Terminal's typed prefix from.
    ///
    /// Content that depends on the clock must be substituted at layout time
    /// (this field is written at the end of `paint` for the *next* frame's
    /// layout to read), never from inside `paint` itself: `LabelRun::set_content`
    /// drops the cached shape immediately (`text.rs`), and `LabelRun::paint` is
    /// a no-op until the next `layout` reshapes it — so a paint-only frame
    /// (no relayout between) would paint nothing for a run mutated in the same
    /// paint call that reads it. Shaping from this cached value instead means
    /// the layout that ran earlier in the same frame always leaves `paint` with
    /// a fresh, non-empty shape, and a paint-only frame simply keeps painting
    /// whatever the last layout produced.
    elapsed: Duration,
    /// The two actions' boxes in the widget's own space.
    actions: [Rect; 2],
    /// The action a `Down` armed.
    armed: Option<usize>,
    /// The action the pointer is over.
    hovered: Option<usize>,
    on_action: ErasedArgCallback<NotFoundAction>,
}

impl NotFoundWidget {
    /// Which style the page is showing.
    pub fn page_style(&self) -> NotFoundStyle {
        self.config.style
    }

    /// How many cells the code is drawn as.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// The box of the action at `index` in [`NotFoundAction`] order.
    pub fn action_rect(&self, action: NotFoundAction) -> Rect {
        self.actions[action_index(action)]
    }

    /// Whether the stage is hovered, as of the last paint.
    pub fn is_hovered(&self) -> bool {
        self.tracker.hovered()
    }

    /// The action `position` lands on, if any.
    fn action_at(&self, position: Point) -> Option<usize> {
        self.actions
            .iter()
            .position(|rect| rect.width() > 0.0 && rect.contains(position))
    }

    /// Whether the style still owes a frame at `elapsed`.
    fn is_running(&self, elapsed: Duration) -> bool {
        match self.config.style {
            NotFoundStyle::Glitch => elapsed < GLITCH_SCRAMBLE,
            NotFoundStyle::Terminal => {
                let last = TERMINAL_LINES[2];
                elapsed
                    < last.1
                        + TERMINAL_CHAR_STAGGER * self.lines[2].content().chars().count() as u32
            }
            _ => false,
        }
    }
}

/// The index an action takes in the widget's own two-slot arrays.
fn action_index(action: NotFoundAction) -> usize {
    match action {
        NotFoundAction::Home => 0,
        NotFoundAction::Browse => 1,
    }
}

/// The action at `index`, the inverse of [`action_index`].
fn action_at_index(index: usize) -> NotFoundAction {
    if index == 0 {
        NotFoundAction::Home
    } else {
        NotFoundAction::Browse
    }
}

/// One shaped run per grapheme of `code`.
fn code_cells(code: &str) -> Vec<LabelRun> {
    code.chars()
        .map(|ch| LabelRun::new(ch.to_string()))
        .collect()
}

/// The terminal's three lines with the code substituted into the third.
fn terminal_lines(code: &str) -> Vec<LabelRun> {
    TERMINAL_LINES
        .iter()
        .map(|(text, _)| LabelRun::new(text.replace("{}", code)))
        .collect()
}

impl<State: 'static> View<State> for NotFoundView<State> {
    type Element = NotFoundWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> NotFoundWidget {
        let cells = code_cells(&self.config.code);
        let lines = terminal_lines(&self.config.code);
        NotFoundWidget {
            scrambled: cells.iter().map(|c| LabelRun::new(c.content())).collect(),
            typed: lines.iter().map(|_| LabelRun::new(String::new())).collect(),
            cells,
            lines,
            title: LabelRun::new(self.config.title.clone()),
            description: LabelRun::new(self.config.description.clone()),
            home: LabelRun::new(self.config.home_label.clone()),
            browse: LabelRun::new(self.config.browse_label.clone()),
            config: self.config.clone(),
            tracker: PointerTracker::new(),
            hover: Lane::at_rest(self.config.style.hover_ramp(), 0.0),
            press: [
                Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
                Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            ],
            started: None,
            elapsed: Duration::ZERO,
            actions: [Rect::ZERO; 2],
            armed: None,
            hovered: None,
            on_action: erase_callback_arg(&self.on_action),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut NotFoundWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.config != self.config {
            if element.config.code != self.config.code || element.config.style != self.config.style
            {
                // A new code, or a new style, is a new run: the scramble and the
                // typing both play again, which is upstream's own trigger.
                element.cells = code_cells(&self.config.code);
                element.scrambled = element
                    .cells
                    .iter()
                    .map(|c| LabelRun::new(c.content()))
                    .collect();
                element.lines = terminal_lines(&self.config.code);
                element.typed = element
                    .lines
                    .iter()
                    .map(|_| LabelRun::new(String::new()))
                    .collect();
                element.started = None;
                element.elapsed = Duration::ZERO;
                element.armed = None;
            }
            element.title.set_content(self.config.title.clone());
            element
                .description
                .set_content(self.config.description.clone());
            element.home.set_content(self.config.home_label.clone());
            element.browse.set_content(self.config.browse_label.clone());
            element.config = self.config.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_action = erase_callback_arg(&self.on_action);
        flags
    }

    fn teardown(&self, _element: &mut NotFoundWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The label family: the theme's own scale, with the catalog's sans stack as
/// the unthemed fallback.
fn family_of(theme: Option<&Theme>) -> frust::authoring::text::FontFamily {
    theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    })
}

/// One label style at `size`.
fn page_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    TextStyle {
        family: family_of(theme),
        ..crate::text::label_style(size)
    }
}

/// The mono style the terminal and the glitch both set.
fn mono_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::mono_family(),
        ..crate::text::label_style(size)
    }
}

impl Widget for NotFoundWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        // Every style is resolved before the first `layout` call: the shaper
        // takes `ctx` mutably, and the theme read borrows it.
        let title_style = page_style(theme, NOT_FOUND_TITLE_SIZE);
        let body_style = page_style(theme, style::TEXT_SM);
        let code_style = if self.config.style == NotFoundStyle::Glitch {
            mono_style(NOT_FOUND_CODE_SIZE)
        } else {
            page_style(theme, NOT_FOUND_CODE_SIZE)
        };
        let terminal_style = mono_style(style::TEXT_SM);

        // The Glitch scramble and the Terminal's typed prefix are substituted
        // here, from `self.elapsed` (what the last `paint` observed), rather
        // than from `paint` itself — see the field doc on `elapsed` for why: a
        // run's content must never change without a layout pass shaping it
        // before the next paint that reads it.
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let elapsed = if reduce {
            // A reduced-motion page is already finished: the scramble has
            // resolved and the terminal has typed itself out (mirrors the
            // same substitution `paint` makes for its own `elapsed`).
            Duration::from_secs(60)
        } else {
            self.elapsed
        };
        match self.config.style {
            NotFoundStyle::Glitch => {
                let settled = glitch_settled(elapsed, self.cells.len());
                let tick = glitch_tick(elapsed);
                for (index, cell) in self.scrambled.iter_mut().enumerate() {
                    let source = self.cells[index].content().to_string();
                    let shown = if index < settled || source == " " {
                        source
                    } else {
                        glitch_glyph(index, tick).to_string()
                    };
                    cell.set_content(shown);
                }
            }
            NotFoundStyle::Terminal => {
                for (index, (_, delay)) in TERMINAL_LINES.into_iter().enumerate() {
                    let stagger = if index == 1 {
                        TERMINAL_ERROR_STAGGER
                    } else {
                        TERMINAL_CHAR_STAGGER
                    };
                    let source: Vec<char> = self.lines[index].content().chars().collect();
                    let typed = terminal_typed(elapsed, delay, stagger, source.len());
                    let text: String = source.iter().take(typed).collect();
                    self.typed[index].set_content(text);
                }
            }
            NotFoundStyle::Magnetic | NotFoundStyle::Spotlight | NotFoundStyle::Stacked => {}
        }

        self.title.layout(ctx, &title_style);
        self.description.layout(ctx, &body_style);
        self.home.layout(ctx, &body_style);
        self.browse.layout(ctx, &body_style);
        for cell in self.cells.iter_mut().chain(self.scrambled.iter_mut()) {
            cell.layout(ctx, &code_style);
        }
        for line in self.lines.iter_mut().chain(self.typed.iter_mut()) {
            line.layout(ctx, &terminal_style);
        }

        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            SPOTLIGHT_MAX_WIDTH
        };
        let height = NOT_FOUND_STAGE_HEIGHT.max(bc.min().height);

        // The two actions, centred as a row at the stage's foot.
        let home = self.home.size().width + NOT_FOUND_ACTION_PADDING_X * 2.0;
        let browse = self.browse.size().width + NOT_FOUND_ACTION_PADDING_X * 2.0;
        let row = home + NOT_FOUND_ACTION_GAP + browse;
        let mut x = (width - row) / 2.0;
        let y = height - NOT_FOUND_STAGE_GAP - NOT_FOUND_ACTION_HEIGHT;
        self.actions[0] =
            Rect::from_origin_size(Point::new(x, y), Size::new(home, NOT_FOUND_ACTION_HEIGHT));
        x += home + NOT_FOUND_ACTION_GAP;
        self.actions[1] =
            Rect::from_origin_size(Point::new(x, y), Size::new(browse, NOT_FOUND_ACTION_HEIGHT));

        self.tracker.set_size(Size::new(width, height));
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let (accent, on_accent) = match theme {
            Some(t) => (t.scheme().primary, t.scheme().on_primary),
            None => (
                crate::BEUI_LIGHT.primary,
                crate::BEUI_LIGHT.primary_foreground,
            ),
        };
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        // The framework's hover answer is authoritative; a stale latch would
        // leave a deck fanned under a pointer that has left.
        self.tracker.sync_hovered(ctx.is_hovered());
        self.hover.retarget_with(
            self.config.style.hover_ramp(),
            if self.tracker.hovered() && !reduce {
                1.0
            } else {
                0.0
            },
        );
        if reduce {
            self.hover.snap();
            for lane in &mut self.press {
                lane.snap();
            }
        } else {
            let mut animating = self.hover.advance(now);
            for lane in &mut self.press {
                animating |= lane.advance(now);
            }
            if animating {
                ctx.request_frame();
            }
        }

        let started = *self.started.get_or_insert(now);
        let elapsed = if reduce {
            // A reduced-motion page is already finished: the scramble has
            // resolved and the terminal has typed itself out.
            Duration::from_secs(60)
        } else {
            now.saturating_sub(started)
        };
        // Latched for the *next* layout pass to shape the Glitch scramble's
        // and the Terminal's typed prefix from — never mutated here, see the
        // `elapsed` field doc.
        self.elapsed = elapsed;
        if !reduce && self.is_running(elapsed) {
            // The scramble/typing is still substituting content, which only a
            // layout pass may reshape (`request_layout` implies a frame too).
            ctx.request_layout();
        }
        if !reduce && self.config.style == NotFoundStyle::Terminal {
            // The caret is a decorative loop, not a transition — the frame gate
            // is allowed to throttle it.
            ctx.request_frame_class(TickClass::CosmeticLoop);
        }

        // The code band: the top third of the stage, above the copy.
        let band = Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(
                size.width,
                (size.height
                    - NOT_FOUND_ACTION_HEIGHT
                    - self.title.size().height
                    - self.description.size().height
                    - NOT_FOUND_COPY_GAP
                    - NOT_FOUND_STAGE_GAP * 3.0)
                    .max(0.0),
            ),
        );
        match self.config.style {
            NotFoundStyle::Glitch => self.paint_glitch(scene, origin, band, chrome.ink),
            NotFoundStyle::Magnetic => self.paint_magnetic(scene, origin, band, chrome.ink),
            NotFoundStyle::Spotlight => self.paint_spotlight(scene, origin, band),
            NotFoundStyle::Stacked => self.paint_stack(
                scene,
                origin,
                band,
                chrome.ink,
                chrome.dim_ink,
                chrome.surface,
                chrome.border,
            ),
            NotFoundStyle::Terminal => self.paint_terminal(scene, origin, band, elapsed),
        }

        // The copy band.
        let title = self.title.size();
        let copy_y = band.y1 + NOT_FOUND_STAGE_GAP;
        self.title.paint(
            origin + Vec2::new((size.width - title.width) / 2.0, copy_y),
            chrome.ink,
            scene,
        );
        let body = self.description.size();
        self.description.paint(
            origin
                + Vec2::new(
                    (size.width - body.width) / 2.0,
                    copy_y + title.height + NOT_FOUND_COPY_GAP,
                ),
            chrome.dim_ink,
            scene,
        );

        // The two actions.
        for index in 0..2 {
            let rect = self.actions[index];
            let hovered = self.hovered == Some(index);
            let scale = press_scale(NOT_FOUND_PRESS_SCALE, self.press[index].value())
                * if hovered && !reduce {
                    NOT_FOUND_HOVER_SCALE
                } else {
                    1.0
                };
            let box_size = Size::new(rect.width() * scale, rect.height() * scale);
            let at = origin
                + Vec2::new(
                    rect.x0 + (rect.width() - box_size.width) / 2.0,
                    rect.y0 + (rect.height() - box_size.height) / 2.0,
                );
            let (fill, ink) = if index == 0 {
                (accent, on_accent)
            } else {
                (chrome.surface, chrome.ink)
            };
            scene.fill_rounded_rect(at, box_size, style::RADIUS_CONTROL, fill);
            if index == 1 {
                paint_panel_hairline(scene, at, box_size, style::RADIUS_CONTROL, chrome.border);
            }
            let label = if index == 0 { &self.home } else { &self.browse };
            let text = label.size();
            label.paint(
                at + Vec2::new(
                    (box_size.width - text.width) / 2.0,
                    (box_size.height - text.height) / 2.0,
                ),
                ink,
                scene,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                // Focus lands on the page, and the primary action is what a
                // bare activation means — upstream's own tab order puts it
                // first.
                (self.on_action)(ctx, NotFoundAction::Home);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let title = self.title.content().to_string();
        let code = self.config.code.clone();
        let description = self.description.content().to_string();
        let home = self.home.content().to_string();
        let browse = self.browse.content().to_string();
        ctx.push_container(
            Role::Group,
            move |node| {
                node.set_label(title.as_str());
            },
            |ctx| {
                // The code is announced as itself, never as whatever glyph the
                // scramble happens to be showing — upstream's own `aria-label`.
                ctx.push_node(Role::Label, |node| {
                    node.set_label(code.as_str());
                });
                ctx.push_node(Role::Label, |node| {
                    node.set_label(description.as_str());
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label(home.as_str());
                    node.add_action(Action::Click);
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label(browse.as_str());
                    node.add_action(Action::Click);
                });
            },
        );
    }
}

impl NotFoundWidget {
    /// The code's own width once shaped, in logical px, with `overlap` taken out
    /// between consecutive cells.
    fn code_width(&self, overlap: f64) -> f64 {
        let total: f64 = self.cells.iter().map(|cell| cell.size().width).sum();
        (total - overlap * self.cells.len().saturating_sub(1) as f64).max(0.0)
    }

    /// `glitch.tsx`: the scrambling code under two chromatic ghosts.
    ///
    /// The substituted glyphs are already shaped in `self.scrambled` — this
    /// only paints them (see `Widget::layout`, which substitutes and reshapes
    /// them from `self.elapsed` before every paint that could read them).
    fn paint_glitch(&mut self, scene: &mut dyn PaintScene, origin: Point, band: Rect, ink: Color) {
        let width = self.code_width(0.0);
        let height = self.cells.first().map_or(0.0, |cell| cell.size().height);
        let base = origin
            + Vec2::new(
                band.x0 + (band.width() - width) / 2.0,
                band.y0 + (band.height() - height) / 2.0,
            );
        let split = self.hover.value().clamp(0.0, 1.0);
        // The ghosts first, then the code itself over them.
        for (shift, colour) in [
            (GLITCH_GHOST_SHIFT, GLITCH_GHOST_WARM),
            (-GLITCH_GHOST_SHIFT, GLITCH_GHOST_COOL),
        ] {
            if split <= 0.0 {
                continue;
            }
            let tint = style::with_alpha(colour, GLITCH_GHOST_ALPHA * split as f32);
            self.paint_code_row(scene, base + Vec2::new(shift * split, 0.0), tint, 0.0);
        }
        self.paint_code_row(scene, base, ink, 0.0);
    }

    /// Draw the scrambled cells in a row from `at`, `overlap` px tighter than
    /// their own advance.
    fn paint_code_row(&self, scene: &mut dyn PaintScene, at: Point, ink: Color, overlap: f64) {
        let mut x = at.x;
        for cell in &self.scrambled {
            cell.paint(Point::new(x, at.y), ink, scene);
            x += cell.size().width - overlap;
        }
    }

    /// `magnetic.tsx`: each digit pulled toward the cursor and springing back.
    fn paint_magnetic(
        &mut self,
        scene: &mut dyn PaintScene,
        origin: Point,
        band: Rect,
        ink: Color,
    ) {
        // The scrambled row is the paint surface for every character style, so
        // it carries the plain code here.
        for (index, cell) in self.scrambled.iter_mut().enumerate() {
            let source = self.cells[index].content().to_string();
            cell.set_content(source);
        }
        let width = self.code_width(MAGNETIC_OVERLAP);
        let height = self.cells.first().map_or(0.0, |cell| cell.size().height);
        let mut x = band.x0 + (band.width() - width) / 2.0;
        let y = band.y0 + (band.height() - height) / 2.0;
        for cell in &self.scrambled {
            let cell_size = cell.size();
            // The pull is measured from each digit's own centre, so the digit
            // under the cursor barely moves and its neighbours lean in.
            let centre = Point::new(x + cell_size.width / 2.0, y + cell_size.height / 2.0);
            let pull = if self.tracker.hovered() {
                (self.tracker.position() - centre.to_vec2()).to_vec2() * MAGNETIC_STRENGTH
            } else {
                Vec2::ZERO
            };
            cell.paint(origin + Vec2::new(x, y) + pull, ink, scene);
            x += cell_size.width - MAGNETIC_OVERLAP;
        }
    }

    /// `spotlight.tsx`: a dim code on a dark panel, revealed bright inside a
    /// cursor-tracked circle.
    fn paint_spotlight(&mut self, scene: &mut dyn PaintScene, origin: Point, band: Rect) {
        for (index, cell) in self.scrambled.iter_mut().enumerate() {
            let source = self.cells[index].content().to_string();
            cell.set_content(source);
        }
        let panel_width = band.width().min(SPOTLIGHT_MAX_WIDTH);
        let panel_height = (panel_width / SPOTLIGHT_ASPECT).min(band.height());
        let panel = Rect::from_origin_size(
            Point::new(
                band.x0 + (band.width() - panel_width) / 2.0,
                band.y0 + (band.height() - panel_height) / 2.0,
            ),
            Size::new(panel_width, panel_height),
        );
        let at = origin + panel.origin().to_vec2();
        scene.fill_rounded_rect(at, panel.size(), style::RADIUS_3XL, NOT_FOUND_PANEL_INK);

        let width = self.code_width(0.0);
        let height = self.cells.first().map_or(0.0, |cell| cell.size().height);
        let code_at = at
            + Vec2::new(
                (panel.width() - width) / 2.0,
                (panel.height() - height) / 2.0,
            );
        self.paint_code_row(
            scene,
            code_at,
            style::with_alpha(Color::WHITE, SPOTLIGHT_DIM_ALPHA),
            0.0,
        );

        // The bright layer, clipped to the spotlight. A rounded clip whose
        // radius is half its own extent is a circle, which is the nearest this
        // vocabulary comes to the radial mask (see the module docs).
        let spot = if self.tracker.hovered() {
            self.tracker.position()
        } else {
            panel.center()
        };
        let spot_at = origin
            + Vec2::new(
                spot.x - SPOTLIGHT_RADIUS / 2.0,
                spot.y - SPOTLIGHT_RADIUS / 2.0,
            );
        scene.push_clip_rounded(
            spot_at,
            Size::new(SPOTLIGHT_RADIUS, SPOTLIGHT_RADIUS),
            SPOTLIGHT_RADIUS / 2.0,
        );
        self.paint_code_row(scene, code_at, Color::WHITE, 0.0);
        scene.pop_clip();
    }

    /// `stacked.tsx`: the code card over two hidden cards that fan on hover.
    #[allow(clippy::too_many_arguments)]
    fn paint_stack(
        &mut self,
        scene: &mut dyn PaintScene,
        origin: Point,
        band: Rect,
        ink: Color,
        dim: Color,
        surface: Color,
        border: Color,
    ) {
        for (index, cell) in self.scrambled.iter_mut().enumerate() {
            let source = self.cells[index].content().to_string();
            cell.set_content(source);
        }
        let deck = Rect::from_origin_size(
            Point::new(
                band.x0 + (band.width() - STACK_WIDTH) / 2.0,
                band.y0 + (band.height() - STACK_HEIGHT) / 2.0,
            ),
            Size::new(STACK_WIDTH, STACK_HEIGHT),
        );
        let fan = self.hover.value().clamp(0.0, 1.0);
        for slot in 0..3 {
            let (offset, rotation) = stacked_card(slot, fan);
            let at = origin + deck.origin().to_vec2() + offset;
            scene.push_transform(frust::authoring::Affine::rotate_about(
                rotation.to_radians(),
                Point::new(at.x + deck.width() / 2.0, at.y + deck.height() / 2.0),
            ));
            scene.fill_rounded_rect(at, deck.size(), STACK_RADIUS, surface);
            paint_panel_hairline(scene, at, deck.size(), STACK_RADIUS, border);
            if slot == 2 {
                let width = self.code_width(0.0);
                let height = self.cells.first().map_or(0.0, |cell| cell.size().height);
                self.paint_code_row(
                    scene,
                    at + Vec2::new(
                        (deck.width() - width) / 2.0,
                        (deck.height() - height) / 2.0 - style::GAP_MD,
                    ),
                    ink,
                    0.0,
                );
                let _ = dim;
            }
            scene.pop_transform();
        }
    }

    /// `terminal.tsx`: a window typing three lines under a blinking caret.
    fn paint_terminal(
        &mut self,
        scene: &mut dyn PaintScene,
        origin: Point,
        band: Rect,
        elapsed: Duration,
    ) {
        let width = band.width().min(TERMINAL_MAX_WIDTH);
        let height = (TERMINAL_BAR_HEIGHT + TERMINAL_LINE_HEIGHT * 3.0 + style::GAP_MD * 2.0)
            .min(band.height());
        let window = Rect::from_origin_size(
            Point::new(
                band.x0 + (band.width() - width) / 2.0,
                band.y0 + (band.height() - height) / 2.0,
            ),
            Size::new(width, height),
        );
        let at = origin + window.origin().to_vec2();
        scene.fill_rounded_rect(at, window.size(), style::RADIUS_XL, NOT_FOUND_PANEL_INK);

        // The title bar's three dots.
        for (index, dot) in TERMINAL_DOTS.into_iter().enumerate() {
            scene.fill_rounded_rect(
                Point::new(
                    at.x + FLAP_DOT_INSET + index as f64 * (TERMINAL_DOT_SIZE + style::GAP_SM),
                    at.y + (TERMINAL_BAR_HEIGHT - TERMINAL_DOT_SIZE) / 2.0,
                ),
                Size::new(TERMINAL_DOT_SIZE, TERMINAL_DOT_SIZE),
                TERMINAL_DOT_SIZE / 2.0,
                dot,
            );
        }
        scene.fill_rect(
            Point::new(at.x, at.y + TERMINAL_BAR_HEIGHT),
            Size::new(window.width(), style::BORDER_WIDTH),
            style::with_alpha(Color::WHITE, TERMINAL_HAIRLINE_ALPHA),
        );

        // The typed prefix is already shaped in `self.typed` — this only
        // paints it (see `Widget::layout`, which substitutes and reshapes it
        // from `self.elapsed` before every paint that could read it). The
        // caret's blink alone still reads the live `elapsed` below: it never
        // changes what is shaped, only whether the same rect is drawn.
        for index in 0..TERMINAL_LINES.len() {
            let line_at = Point::new(
                at.x + FLAP_DOT_INSET,
                at.y + TERMINAL_BAR_HEIGHT + style::GAP_MD + index as f64 * TERMINAL_LINE_HEIGHT,
            );
            let ink = if index == 1 {
                TERMINAL_DOTS[0]
            } else {
                style::with_alpha(Color::WHITE, TERMINAL_TEXT_ALPHA)
            };
            self.typed[index].paint(line_at, ink, scene);
            if index == 2 && terminal_caret_lit(elapsed) {
                scene.fill_rect(
                    Point::new(line_at.x + self.typed[index].size().width + 2.0, line_at.y),
                    Size::new(TERMINAL_CARET_WIDTH, TERMINAL_LINE_HEIGHT * 0.8),
                    style::with_alpha(Color::WHITE, TERMINAL_TEXT_ALPHA),
                );
            }
        }
    }

    /// The `Widget::event` pointer arm: the two actions own their presses, the
    /// stage tracks the pointer for the magnetic and spotlight styles.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        if self.tracker.on_pointer(p, size) && self.config.style.tracks_pointer() {
            ctx.request_redraw();
        }
        let over = self.action_at(p.position);
        match p.phase {
            PointerPhase::Move => {
                if inside(p.position, size) {
                    ctx.claim_hover();
                }
                if over.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                ctx.request_focus();
                let Some(index) = over else {
                    return EventResult::Ignored;
                };
                ctx.capture_pointer();
                self.armed = Some(index);
                self.press[index].retarget(1.0);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                self.press[armed].retarget(0.0);
                ctx.request_redraw();
                if over == Some(armed) {
                    (self.on_action)(ctx, action_at_index(armed));
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                self.press[armed].retarget(0.0);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

/// `px-4` — how far in from the window's edge the dots and the lines start.
const FLAP_DOT_INSET: f64 = 16.0;

/// `h-3 w-3` — one traffic-light dot's box, in logical px.
const TERMINAL_DOT_SIZE: f64 = 12.0;

/// `border-white/10` — the title bar's rule.
const TERMINAL_HAIRLINE_ALPHA: f32 = 0.10;

/// `text-white/80` — the typed text's alpha.
const TERMINAL_TEXT_ALPHA: f32 = 0.80;

/// `w-[0.55ch]` — the caret's width, in logical px.
const TERMINAL_CARET_WIDTH: f64 = 7.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(700.0, 520.0);

    // ---- The glitch scramble ------------------------------------------------

    /// The defining property: characters settle left to right, and everything
    /// has settled by the end of the run.
    #[test]
    fn the_scramble_settles_its_characters_in_reading_order() {
        assert_eq!(glitch_settled(Duration::ZERO, 3), 0);
        // 700ms over three characters: one settles every ~233ms.
        assert_eq!(glitch_settled(Duration::from_millis(200), 3), 0);
        assert_eq!(glitch_settled(Duration::from_millis(240), 3), 1);
        assert_eq!(glitch_settled(Duration::from_millis(480), 3), 2);
        assert_eq!(glitch_settled(GLITCH_SCRAMBLE, 3), 3);
        assert_eq!(glitch_settled(Duration::from_secs(30), 3), 3);
        // The count never runs past the code, however long the page sits.
        assert_eq!(glitch_settled(Duration::from_secs(30), 0), 0);
    }

    #[test]
    fn a_glyph_holds_for_a_whole_tick_and_then_changes() {
        assert_eq!(glitch_tick(Duration::ZERO), 0);
        assert_eq!(glitch_tick(GLITCH_TICK - Duration::from_millis(1)), 0);
        assert_eq!(glitch_tick(GLITCH_TICK), 1);
        assert_eq!(glitch_tick(GLITCH_TICK * 4), 4);
        // The same cell on the same tick is the same glyph...
        assert_eq!(glitch_glyph(1, 7), glitch_glyph(1, 7));
        // ...and the alphabet is the ported one.
        let glyphs: Vec<char> = GLITCH_GLYPHS.chars().collect();
        for index in 0..8usize {
            for tick in 0..8u64 {
                assert!(glyphs.contains(&glitch_glyph(index, tick)));
            }
        }
    }

    /// The scramble has to *look* random: consecutive ticks and neighbouring
    /// cells must not lock onto one glyph.
    #[test]
    fn the_deterministic_scramble_still_churns() {
        let over_time: std::collections::HashSet<char> =
            (0..20u64).map(|tick| glitch_glyph(0, tick)).collect();
        assert!(over_time.len() > 8, "one cell barely moved: {over_time:?}");
        let across_cells: std::collections::HashSet<char> =
            (0..20usize).map(|index| glitch_glyph(index, 3)).collect();
        assert!(
            across_cells.len() > 8,
            "a whole row locked together: {across_cells:?}"
        );
    }

    // ---- The card stack -----------------------------------------------------

    #[test]
    fn the_deck_is_flat_at_rest_and_mirrors_itself_when_fanned() {
        for slot in 0..3 {
            let (offset, rotation) = stacked_card(slot, 0.0);
            assert_eq!(offset, Vec2::ZERO, "slot {slot} moved at rest");
            assert_eq!(rotation, 0.0);
        }
        let (left, left_rotation) = stacked_card(0, 1.0);
        let (right, right_rotation) = stacked_card(1, 1.0);
        assert_eq!(left.x, -right.x);
        assert_eq!(left.y, right.y, "both back cards slip the same way down");
        assert_eq!(left_rotation, -right_rotation);
        assert_eq!(left_rotation, -STACK_ROTATION);
        // The front card only lifts, and never turns.
        let (front, front_rotation) = stacked_card(2, 1.0);
        assert_eq!(front, Vec2::new(0.0, -STACK_LIFT));
        assert_eq!(front_rotation, 0.0);
        // A slot past the deck takes the inert front rule.
        assert_eq!(stacked_card(9, 1.0), stacked_card(2, 1.0));
    }

    #[test]
    fn a_half_fanned_deck_is_half_way_out_and_the_progress_clamps() {
        let (half, rotation) = stacked_card(0, 0.5);
        assert_eq!(half.x, -STACK_SPREAD_X / 2.0);
        assert_eq!(rotation, -STACK_ROTATION / 2.0);
        assert_eq!(stacked_card(0, 5.0), stacked_card(0, 1.0));
        assert_eq!(stacked_card(0, -5.0), stacked_card(0, 0.0));
    }

    // ---- The terminal -------------------------------------------------------

    #[test]
    fn a_line_types_one_character_per_stagger_after_its_own_delay() {
        let stagger = Duration::from_millis(10);
        let delay = Duration::from_millis(100);
        assert_eq!(terminal_typed(Duration::ZERO, delay, stagger, 5), 0);
        assert_eq!(
            terminal_typed(Duration::from_millis(99), delay, stagger, 5),
            0
        );
        assert_eq!(
            terminal_typed(Duration::from_millis(100), delay, stagger, 5),
            0
        );
        assert_eq!(
            terminal_typed(Duration::from_millis(125), delay, stagger, 5),
            2
        );
        // It stops at the end of the line rather than running past it.
        assert_eq!(
            terminal_typed(Duration::from_secs(10), delay, stagger, 5),
            5
        );
        // A zero stagger is an instant line, not a division by zero.
        assert_eq!(
            terminal_typed(Duration::from_millis(200), delay, Duration::ZERO, 5),
            5
        );
    }

    #[test]
    fn the_lines_start_in_the_order_upstream_delays_them() {
        let delays: Vec<Duration> = TERMINAL_LINES.iter().map(|(_, delay)| *delay).collect();
        for pair in delays.windows(2) {
            assert!(pair[0] < pair[1], "out of order: {delays:?}");
        }
        // The error line types faster than the prompts either side of it.
        assert!(TERMINAL_ERROR_STAGGER < TERMINAL_CHAR_STAGGER);
    }

    #[test]
    fn the_caret_is_lit_for_the_first_half_of_every_cycle() {
        assert!(terminal_caret_lit(Duration::ZERO));
        assert!(terminal_caret_lit(TERMINAL_CARET_PERIOD.mul_f64(0.49)));
        assert!(!terminal_caret_lit(TERMINAL_CARET_PERIOD.mul_f64(0.51)));
        assert!(!terminal_caret_lit(TERMINAL_CARET_PERIOD.mul_f64(0.99)));
        // ...and it cycles rather than settling.
        assert!(terminal_caret_lit(TERMINAL_CARET_PERIOD));
        assert!(terminal_caret_lit(TERMINAL_CARET_PERIOD.mul_f64(4.1)));
    }

    // ---- The mounted page ---------------------------------------------------

    #[derive(Default)]
    struct App {
        actions: Vec<NotFoundAction>,
        style: NotFoundStyle,
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        clock: f64,
    }

    impl Harness {
        fn new(style: NotFoundStyle) -> Self {
            Self::themed(style, light())
        }

        fn themed(style: NotFoundStyle, theme: Theme) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    style,
                    ..App::default()
                },
                tcx: TextContext::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(theme));
            h.step(0.0);
            h
        }

        fn step(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut logic = move |s: &mut App| {
                frust::Stack(vec![any(not_found()
                    .style(s.style)
                    .on_action(|s: &mut App, action| s.actions.push(action)))])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            let now = self.clock;
            self.root.paint(&mut rec, ft_ms(now));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// The centre of the action at `index`, in window space — the actions
        /// are the two `h-11` rounded rects the page paints last.
        fn action_centre(&mut self, index: usize) -> Point {
            let rec = self.step(0.0);
            let boxes: Vec<Rect> = rec
                .rrects
                .iter()
                .filter(|(_, size, _, _)| size.height == NOT_FOUND_ACTION_HEIGHT)
                .map(|(origin, size, _, _)| Rect::from_origin_size(*origin, *size))
                .collect();
            boxes[index].center()
        }
    }

    #[test]
    fn every_style_paints_its_stage_its_copy_and_both_actions() {
        for style in NotFoundStyle::ALL {
            let mut h = Harness::new(style);
            // 600ms in: the scramble is still mid-run and two of the
            // terminal's three lines have started typing, so every style's
            // own code/line treatment has produced its own glyph runs on top
            // of the shared title/description/home/browse labels — a bare
            // `!rec.inks.is_empty()` would already pass on the four labels
            // alone and miss a style that paints none of its own code. Two
            // steps at the same clock: the first is what latches `elapsed`
            // for `layout` to substitute from (see the field doc), the
            // second is the real (layout + paint) frame that reads it.
            h.step(600.0);
            let rec = h.step(0.0);
            let actions = rec
                .rrects
                .iter()
                .filter(|(_, size, _, _)| size.height == NOT_FOUND_ACTION_HEIGHT)
                .count();
            assert_eq!(actions, 2, "{style:?} lost an action");

            // The title, description and both action labels are the floor
            // every style shares (>= 1 run each, however a label's own
            // shaping splits); each style's own code/line treatment adds its
            // own runs on top — the code is `NOT_FOUND_CODE` ("404", 3
            // cells), and the spotlight paints it twice (a dim base layer
            // plus a bright layer clipped to the spotlight).
            let code_runs = match style {
                NotFoundStyle::Spotlight => 6,
                NotFoundStyle::Terminal => 2,
                NotFoundStyle::Glitch | NotFoundStyle::Magnetic | NotFoundStyle::Stacked => 3,
            };
            assert!(
                rec.inks.len() >= 4 + code_runs,
                "{style:?} drew {} glyph runs, expected at least {}",
                rec.inks.len(),
                4 + code_runs
            );
        }
    }

    /// M2 regression: `LabelRun::set_content` (`text.rs`) drops the cached
    /// shape it is called on, and `LabelRun::paint` is a no-op until the next
    /// `layout` reshapes it — so mutating a run's content from `paint` itself
    /// paints nothing on a paint-only frame (no `layout` between). The
    /// Terminal's typed lines and the Glitch scramble's cells must instead be
    /// substituted in `layout`, so a paint-only frame keeps painting whatever
    /// the last layout shaped.
    #[test]
    fn a_paint_only_frame_after_typing_has_begun_still_paints_the_prefix_and_advances_the_caret() {
        let mut h = Harness::new(NotFoundStyle::Terminal);

        /// The caret's own `x`, identified by its distinctive `w-[0.55ch]`
        /// size — the only rect this widget fills at that width.
        fn caret_x(rec: &Recorder) -> f64 {
            rec.rects
                .iter()
                .find(|(_, size, _)| size.width == TERMINAL_CARET_WIDTH)
                .expect("no caret drawn")
                .0
                .x
        }

        // At rest nothing is typed yet, so the caret sits right after the
        // prompt — the baseline `set_content` used to leave it stuck at.
        let baseline = caret_x(&h.step(0.0));

        // Two real (layout + paint) frames at the same clock, well past the
        // third line's typing delay and lit within the caret's own blink
        // cycle: the first latches `elapsed` for `layout` to substitute from
        // (see the field doc), the second is the frame that actually reads
        // the freshly-shaped run — these are the only frames here allowed to
        // relayout.
        h.step(2_200.0);
        let typing = h.step(0.0);
        assert!(
            !typing.inks.is_empty(),
            "the typing frame painted no glyphs"
        );

        // Paint-only frames after that: no rebuild, no layout — the
        // production frame shape `request_frame`/`request_frame_class` drive,
        // and exactly the shape the M2 bug's harness never exercised.
        let (paint_only, _) = paint_frame(&mut h, 0.0);
        assert!(
            !paint_only.inks.is_empty(),
            "a paint-only frame after typing began painted no glyphs"
        );
        let advanced = caret_x(&paint_only);
        assert!(
            advanced > baseline,
            "the caret sat at a fixed x instead of tracking the typed prefix"
        );

        // A second paint-only frame in a row keeps painting the same
        // already-typed prefix — it does not fall back to nothing once the
        // run has gone one frame stale, and the caret does not move without
        // a layout pass to reshape it.
        let (still_painting, _) = paint_frame(&mut h, 100.0);
        assert!(
            !still_painting.inks.is_empty(),
            "a second paint-only frame painted no glyphs"
        );
        assert_eq!(
            caret_x(&still_painting),
            advanced,
            "the caret moved without a layout pass"
        );
    }

    /// M2 regression, the Glitch half: `paint_glitch` used to mutate
    /// `self.scrambled` directly, so a paint-only frame mid-scramble (or
    /// right after it settles) painted nothing for the code row.
    #[test]
    fn paint_only_frames_keep_painting_the_glitch_code_row_mid_scramble_and_after_settle() {
        let mut h = Harness::new(NotFoundStyle::Glitch);

        // Mid-scramble: a real frame lays out the substituted glyphs...
        let mid = h.step(200.0);
        assert!(
            !mid.inks.is_empty(),
            "the mid-scramble frame painted no glyphs"
        );
        // ...and a paint-only frame right after keeps painting them, with the
        // same run count (nothing was dropped, nothing was added).
        let (mid_paint_only, _) = paint_frame(&mut h, 0.0);
        assert!(
            !mid_paint_only.inks.is_empty(),
            "a paint-only frame mid-scramble painted no glyphs"
        );
        assert_eq!(
            mid.inks.len(),
            mid_paint_only.inks.len(),
            "a paint-only frame changed how many glyph runs were drawn"
        );

        // Settle the scramble with one more real frame...
        let settled = h.step(2_000.0);
        assert!(
            !settled.inks.is_empty(),
            "the settled frame painted no glyphs"
        );
        // ...and paint-only frames after settling still paint the resolved
        // code, permanently — not just the one frame the settle step landed
        // on.
        let (settled_paint_only, _) = paint_frame(&mut h, 0.0);
        assert!(
            !settled_paint_only.inks.is_empty(),
            "a paint-only frame after settling painted no glyphs"
        );
        assert_eq!(settled.inks.len(), settled_paint_only.inks.len());
        let (still_settled, _) = paint_frame(&mut h, 500.0);
        assert!(
            !still_settled.inks.is_empty(),
            "a later paint-only frame after settling painted no glyphs"
        );
    }

    #[test]
    fn pressing_an_action_reports_exactly_that_action() {
        let mut h = Harness::new(NotFoundStyle::Stacked);
        let home = h.action_centre(0);
        h.event(pointer(PointerPhase::Down, home.x, home.y));
        h.event(pointer(PointerPhase::Up, home.x, home.y));
        assert_eq!(h.state.actions, vec![NotFoundAction::Home]);

        let browse = h.action_centre(1);
        h.event(pointer(PointerPhase::Down, browse.x, browse.y));
        h.event(pointer(PointerPhase::Up, browse.x, browse.y));
        assert_eq!(
            h.state.actions,
            vec![NotFoundAction::Home, NotFoundAction::Browse]
        );
    }

    #[test]
    fn a_press_that_leaves_or_is_cancelled_never_reaches_the_app() {
        let mut h = Harness::new(NotFoundStyle::Glitch);
        let home = h.action_centre(0);
        h.event(pointer(PointerPhase::Down, home.x, home.y));
        h.event(pointer(PointerPhase::Up, home.x, 5.0));
        assert!(h.state.actions.is_empty());
        h.event(pointer(PointerPhase::Down, home.x, home.y));
        h.event(pointer(PointerPhase::Cancel, home.x, home.y));
        assert!(h.state.actions.is_empty());
    }

    #[test]
    fn the_glitch_resolves_the_code_by_the_end_of_its_run() {
        let mut h = Harness::new(NotFoundStyle::Glitch);
        // Mid-run at least one cell is showing a substituted glyph, so the
        // painted ink count is the same but the run is not settled yet...
        let (_, mid_frame) = paint_frame(&mut h, 100.0);
        assert!(mid_frame, "the scramble owes frames while it runs");
        // ...and past the scramble it stops asking for them.
        let (_, late_frame) = paint_frame(&mut h, 2_000.0);
        assert!(!late_frame, "the scramble kept running after it settled");
    }

    #[test]
    fn a_reduced_motion_page_shows_the_settled_code_and_asks_for_nothing() {
        let mut h = Harness::themed(NotFoundStyle::Glitch, reduced());
        let (first, needs) = paint_frame(&mut h, 0.0);
        assert!(!needs, "a reduced-motion glitch requested a frame");
        let (second, _) = paint_frame(&mut h, 120.0);
        assert_eq!(
            first.inks.len(),
            second.inks.len(),
            "the code changed under reduced motion"
        );
    }

    #[test]
    fn the_terminal_keeps_a_cosmetic_loop_running_for_its_caret() {
        let mut h = Harness::new(NotFoundStyle::Terminal);
        // Long past the typing, the caret alone is what keeps a frame owed —
        // and it is a throttleable cosmetic loop, not a transition.
        let (_, needs) = paint_frame(&mut h, 10_000.0);
        assert!(needs, "the caret stopped blinking");
    }

    #[test]
    fn the_spotlight_clips_its_bright_layer_to_a_circle() {
        let mut h = Harness::new(NotFoundStyle::Spotlight);
        let rec = h.step(0.0);
        let spot = rec
            .clips
            .iter()
            .find(|(_, size, _)| size.width == SPOTLIGHT_RADIUS)
            .expect("the spotlight clip");
        assert_eq!(spot.1.height, SPOTLIGHT_RADIUS, "the spot is square...");
        assert_eq!(spot.2, SPOTLIGHT_RADIUS / 2.0, "...so its clip is a circle");
    }

    #[test]
    fn the_stack_fans_on_hover_and_flattens_when_the_pointer_leaves() {
        let mut h = Harness::new(NotFoundStyle::Stacked);
        let flat = h.step(0.0);
        h.event(pointer(PointerPhase::Move, 350.0, 100.0));
        h.step(0.0);
        let fanned = h.step(1_000.0);
        assert_ne!(
            flat.rrects, fanned.rrects,
            "the deck did not fan under the pointer"
        );
        assert_eq!(fanned.transforms.len(), 3, "one transform per deck card");
    }

    /// One whole frame, with whether it asked for another.
    fn paint_frame(h: &mut Harness, ms: f64) -> (Recorder, bool) {
        h.clock += ms;
        let mut rec = Recorder::default();
        let now = h.clock;
        let outcome = h.root.paint(&mut rec, ft_ms(now));
        (rec, outcome.needs_frame)
    }
}
