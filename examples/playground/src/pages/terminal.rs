//! Terminal section: replay a checked-in byte stream through a **real** VT
//! emulator (the `vt100` crate) into a batched, painted 80x45 character grid,
//! with the replay profile chosen on-page.
//!
//! This section is a **demo of a capability**, not a measurement: it exists to
//! show that a plain frust app can drive a real terminal emulator and paint its
//! screen on a device, at a cadence a terminal UI would actually ship at.
//! Nothing here is timed and nothing here guarantees a cadence — the numbers
//! shown in the readout are live diagnostics for the eye, not results.
//!
//! # Why a real emulator, not a hand-rolled parser
//!
//! A five-escape hand-rolled parser would be a strawman: real terminal UIs
//! carry a full VT emulator, and a grid painted from a toy parser proves
//! nothing about what a real one costs to render. `vt100` (MIT, deps
//! `vte`/`itoa`/`unicode-width` only — no tokio, no PTY, no `std::process`) is
//! the emulator here, and it type-checks for `aarch64-linux-android` /
//! `aarch64-apple-ios`, which is what makes this section runnable on the
//! devices the playground exists for. See `Cargo.toml`'s `vt100` pin.
//!
//! A side-by-side comparison against a known-good incumbent renderer (kterm,
//! the Flutter terminal widget) was **designed but never built**: the fixtures
//! below were authored so both sides could replay byte-identical bytes at
//! identical times, and the escape-sequence subset was kept conservative for
//! exactly that reason (`fixtures/terminal/README.md`). The comparison itself
//! does not exist, so nothing here should be read as one.
//!
//! # Fixtures are data, never generated at runtime
//!
//! Each profile embeds `fixtures/terminal/<profile>.chunks` verbatim
//! (`include_bytes!`) and replays it under one contract:
//!
//! ```text
//! <profile>.chunks   repeat [u32 little-endian length][length bytes]
//!                    no header, no trailer, no padding
//! replay             feed chunk i at t = i * (1000 / hz) ms from start
//! ```
//!
//! Embedded rather than read from disk so Android's asset layout and iOS's
//! bundle layout need no per-platform loader. Cost: ~7.5 MB of fixture bytes in
//! the binary, dominated by `htop` (5.5 MB) and `firehose` (1.9 MB) — acceptable
//! for a testing app; were it ever not, the alternative is a `[features]` split
//! (one feature per profile) rather than shrinking the fixture set.
//!
//! # Repaint coalescing: one signal write per tick, never one per chunk
//!
//! A signal write can never be paced by the framework — `FrameGate`'s pacing
//! gate requires `!signals_dirty`, so every write forces an unpaced frame at
//! panel rate. A terminal that wrote a signal per chunk would therefore run
//! `firehose` at 120 forced frames/second, which is not a design anybody would
//! ship. The coalescing happens **app-side**:
//!
//! - A UI-thread `spawn_local` loop ([`run_feed`]) wakes every
//!   [`REPAINT_INTERVAL`] (1/[`REPAINT_HZ`] = 33.3 ms), feeds **every chunk
//!   whose scheduled time has passed** into the emulator, then writes the
//!   generation signal **once**. No input byte is ever dropped or delayed past
//!   its schedule; only the *repaint* is coalesced.
//! - Between ticks nothing is dirty and nothing requests a frame, so the mobile
//!   frame gate skips. A stopped replay (and the `idle` profile) writes the
//!   signal zero times and therefore produces zero frames — resting at zero
//!   cost is the point, which is also why this page does **not** use
//!   playground's other self-driving idiom, `camera.rs`'s always-request-a-frame
//!   `FrameTicker`: that would paint every frame regardless of whether the
//!   emulator produced anything.
//! - The wake source is a real timer: the loop `await`s a
//!   [`frust::spawn_blocking`] sleep, and both mobile shells drain the UI-thread
//!   local task queue at the top of every tick.
//! - Emulator parsing runs on the UI thread, deliberately — that is where a real
//!   terminal integration would have to survive it.
//!
//! **Changing the cap** is one constant: [`REPAINT_HZ`]. Nothing else derives
//! from it.
//!
//! # Same-style run batching
//!
//! Each glyph run becomes exactly one vello `draw_glyphs` call — the scene layer
//! does no batching of its own — so [`batch_screen`] coalesces consecutive cells
//! sharing a whole [`CellStyle`] (fg, bg, bold, italic, underline, inverse, dim)
//! into **one** shaped run and **one** background rectangle. There is never a
//! draw per cell. Typical run counts per painted frame at 80x45, which the
//! on-page readout shows live and this module's tests pin:
//!
//! | profile | style runs/frame | why |
//! |---|---|---|
//! | `idle` | 0 | nothing painted but the backdrop |
//! | `typing` | <= 45 | one default-style run per non-blank row |
//! | `build-log` | ~132 | 3 visible runs/line: level tag, bold counter, message |
//! | `htop` | ~1700 | a fg+bg pair change every 2 cells, 40 runs/row |
//! | `firehose` | ~44 | one default-style run per line |
//!
//! # Width-derived cell geometry
//!
//! The grid is fixed at [`COLS`]x[`ROWS`] cells (the fixture contract), but the
//! **font size is not**: [`solve_cell_geometry`] measures the shaped monospace
//! advance and solves for the size at which exactly [`COLS`] columns fit the
//! available width (and [`ROWS`] rows the available height, whichever binds
//! first) — never an assumed 0.6em ratio. Leftover space letterboxes. That is
//! what keeps all 80 columns visible on a phone instead of clipping ~18 of them.
//!
//! # Deliberate non-features
//!
//! No cursor, no selection, no scrollback view, no reflow-on-resize, and no
//! `Widget::semantics` impl (a 3600-character label rebuilt every generation
//! would be worse than useless to a screen reader). No PTY and no shell: the
//! bytes are checked-in data, never a live process.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use frust::authoring::text::{
    FontFamily, FontStyle, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Point, Size, View,
    Widget,
};
use frust::{
    Axis, ButtonStyle, Color, Component, EdgeInsets, FlexChild, FlexView, Get, GetUntracked,
    Padding, RwSignal, Set, SizedBox, Theme, any, button, component, inflexible, on_cleanup,
    spawn_blocking, spawn_local, text, use_context,
};

use crate::PlaygroundState;

// ---------------------------------------------------------------------------
// Grid + replay constants
// ---------------------------------------------------------------------------

/// Grid width in cells. Fixed by the fixture contract, never derived from the
/// screen (only the font size is — see the module docs).
pub const COLS: u16 = 80;
/// Grid height in cells. Fixed by the fixture contract (see [`COLS`]).
pub const ROWS: u16 = 45;

/// Cell height as a multiple of the derived font size (CSS unitless
/// `line-height`) — 1.2, the usual terminal row pitch.
///
/// The *measured* line box (what actually spaces the rows) comes back from the
/// shaper, not from this constant: see [`solve_cell_geometry`].
pub const LINE_HEIGHT: f32 = 1.2;

/// Font size used **only** when neither axis of the content box is bounded, i.e.
/// when the width-derived geometry has nothing to solve against (a degenerate
/// parent, or a unit test with unbounded constraints).
pub const FALLBACK_FONT_SIZE: f32 = 10.0;

/// Floor on the derived font size. Reaching it means [`COLS`] columns no longer
/// fit the content box at a size the text engine renders sanely (a viewport
/// under ~192 logical px wide at a 0.6em advance), at which point the grid clips
/// — the honest fix there is a narrower grid plus regenerated fixtures, not a
/// smaller font.
pub const MIN_FONT_SIZE: f32 = 4.0;

/// Reference size the face's advance/line-height ratios are measured at, before
/// solving for the size that fits [`COLS`] columns. Large enough that any
/// per-glyph advance quantization is a rounding error in the ratio; the solver
/// then re-measures at the derived size and shrinks if the real metrics
/// overflow, so the ratio is only ever a starting point.
const PROBE_FONT_SIZE: f32 = 64.0;

/// Measure/shrink iterations [`solve_cell_geometry`] may run before accepting
/// whatever it has. A linear-advance face (every real monospace face here)
/// converges on the first try; the extra passes exist for a face whose advance
/// quantizes non-linearly with size.
const FIT_ITERATIONS: usize = 8;

/// Multiplicative backoff applied from the second shrink pass onward. A face
/// that quantizes its advance (say, up to whole pixels) reports the *same* cell
/// width across a band of sizes, so the ratio-derived step lands on a fixed
/// point it can never escape; a geometric step always makes progress.
const FIT_BACKOFF: f64 = 0.9;

/// Overflow, in logical px per axis, the solver accepts as a fit. A shaper
/// returns advances through `f32`, so a grid solved to *exactly* the content
/// width can measure back a hundred-thousandth of a pixel over it; treating that
/// as an overflow would send the solver chasing float jitter (and, with
/// [`FIT_BACKOFF`], shrink the grid ~10% for it).
const FIT_TOLERANCE: f64 = 0.01;

/// The app-side repaint coalescing cap, in Hz — **the single knob** for the
/// mechanism the module docs describe. 30 Hz is the rate a terminal UI would
/// ship at; the framework cannot pace a signal-driven repaint for us.
pub const REPAINT_HZ: f64 = 30.0;

/// [`REPAINT_HZ`] as a period — derived, so [`REPAINT_HZ`] really is the only
/// knob. The feed loop wakes on this grid, feeds every chunk already due, and
/// publishes at most one generation per wake.
pub const REPAINT_INTERVAL: Duration = Duration::from_nanos((1_000_000_000.0 / REPAINT_HZ) as u64);

/// Probe string whose shaped width, divided by its length, yields the monospace
/// cell advance. Ten ASCII digits: all one cell wide in every monospace face,
/// and long enough that a per-glyph quantization rounding is visible in the
/// average if it ever stops being uniform.
const CELL_PROBE: &str = "0123456789";

// ---------------------------------------------------------------------------
// Palette — literal, not themed
// ---------------------------------------------------------------------------

/// Default foreground (SGR 39 / never-set). A literal rather than a theme token:
/// a terminal's palette is its own contract, and the grid must read identically
/// under either app brightness (the page chrome around it stays themed).
pub const DEFAULT_FG: Color = Color::from_rgb8(0xD0, 0xD0, 0xD0);
/// Default background (SGR 49 / never-set), and the whole-grid backdrop fill.
pub const DEFAULT_BG: Color = Color::from_rgb8(0x10, 0x10, 0x10);

/// The 16 ANSI palette entries (`Color::Idx(0..=15)`), xterm's classic values:
/// 0-7 normal, 8-15 bright.
const ANSI_16: [Color; 16] = [
    Color::from_rgb8(0x00, 0x00, 0x00),
    Color::from_rgb8(0xCD, 0x00, 0x00),
    Color::from_rgb8(0x00, 0xCD, 0x00),
    Color::from_rgb8(0xCD, 0xCD, 0x00),
    Color::from_rgb8(0x00, 0x00, 0xEE),
    Color::from_rgb8(0xCD, 0x00, 0xCD),
    Color::from_rgb8(0x00, 0xCD, 0xCD),
    Color::from_rgb8(0xE5, 0xE5, 0xE5),
    Color::from_rgb8(0x7F, 0x7F, 0x7F),
    Color::from_rgb8(0xFF, 0x00, 0x00),
    Color::from_rgb8(0x00, 0xFF, 0x00),
    Color::from_rgb8(0xFF, 0xFF, 0x00),
    Color::from_rgb8(0x5C, 0x5C, 0xFF),
    Color::from_rgb8(0xFF, 0x00, 0xFF),
    Color::from_rgb8(0x00, 0xFF, 0xFF),
    Color::from_rgb8(0xFF, 0xFF, 0xFF),
];

/// Alpha applied to the foreground for SGR 2 (dim). Not exercised by the fixture
/// subset — carried so an out-of-subset byte renders rather than vanishing.
const DIM_ALPHA: f32 = 0.66;

/// Underline stroke width in logical px, and its offset below the cell's top
/// edge as a fraction of the cell height.
const UNDERLINE_WIDTH: f64 = 1.0;
const UNDERLINE_Y_FRACTION: f64 = 0.92;

// ---------------------------------------------------------------------------
// Profiles — one per checked-in fixture
// ---------------------------------------------------------------------------

/// The five replay profiles, mirroring `fixtures/terminal/README.md`'s table.
/// Each variant's bytes are embedded from that directory; `hz` and
/// `chunk_count` are the matching `<profile>.json` manifest values, asserted
/// against the embedded bytes by this module's tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// No output at all — rest cost. The frame gate should idle at zero frames.
    Idle,
    /// One printable char per chunk at 5 Hz — the minimum update (one cell).
    Typing,
    /// One scrolling log line per chunk at 30 Hz — the realistic workload.
    BuildLog,
    /// Full-screen redraw with a colour pair change every 2 cells at 10 Hz —
    /// the style-density axis (~1800 runs/frame).
    Htop,
    /// 8 lines per chunk at 120 Hz (960 lines/s) at ~1 run/line — the rate axis.
    Firehose,
}

impl Profile {
    /// Every profile, in fixture-README order.
    pub const ALL: [Profile; 5] = [
        Profile::Idle,
        Profile::Typing,
        Profile::BuildLog,
        Profile::Htop,
        Profile::Firehose,
    ];

    /// The fixture file stem, and the on-screen button label.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Idle => "idle",
            Profile::Typing => "typing",
            Profile::BuildLog => "build-log",
            Profile::Htop => "htop",
            Profile::Firehose => "firehose",
        }
    }

    /// One line describing what this profile stresses — shown under the profile
    /// row so the buttons are self-explanatory on a device with no docs open.
    pub fn blurb(self) -> &'static str {
        match self {
            Profile::Idle => "no output at all: zero generations, zero frames after mount",
            Profile::Typing => "one printable char per chunk at 5 Hz: one cell changes",
            Profile::BuildLog => "one scrolling log line per chunk at 30 Hz: realistic work",
            Profile::Htop => "full redraw, colour pair every 2 cells at 10 Hz: style density",
            Profile::Firehose => "8 lines per chunk at 120 Hz: raw rate, ~1 run per line",
        }
    }

    /// Chunk emission rate in Hz (`0` for [`Idle`](Self::Idle), which has no
    /// chunks).
    pub fn hz(self) -> u32 {
        match self {
            Profile::Idle => 0,
            Profile::Typing => 5,
            Profile::BuildLog => 30,
            Profile::Htop => 10,
            Profile::Firehose => 120,
        }
    }

    /// The manifest's `chunk_count`. Walking [`chunks`](Self::chunks) must yield
    /// exactly this many chunks and consume every byte — the fixture integrity
    /// gate this module's tests enforce.
    pub fn chunk_count(self) -> usize {
        match self {
            Profile::Idle => 0,
            Profile::Typing => 150,
            Profile::BuildLog => 900,
            Profile::Htop => 300,
            Profile::Firehose => 3600,
        }
    }

    /// The embedded fixture bytes (see the module docs for why they are embedded
    /// rather than read from disk, and what that costs).
    pub fn chunks(self) -> &'static [u8] {
        match self {
            Profile::Idle => include_bytes!("../../fixtures/terminal/idle.chunks"),
            Profile::Typing => include_bytes!("../../fixtures/terminal/typing.chunks"),
            Profile::BuildLog => include_bytes!("../../fixtures/terminal/build-log.chunks"),
            Profile::Htop => include_bytes!("../../fixtures/terminal/htop.chunks"),
            Profile::Firehose => include_bytes!("../../fixtures/terminal/firehose.chunks"),
        }
    }

    /// Milliseconds between consecutive chunks (`1000 / hz`), or `None` for a
    /// profile with no chunks.
    fn interval_ms(self) -> Option<f64> {
        match self.hz() {
            0 => None,
            hz => Some(1000.0 / f64::from(hz)),
        }
    }
}

// ---------------------------------------------------------------------------
// Chunk stream reader (pure)
// ---------------------------------------------------------------------------

/// Read the next `[u32 le length][length bytes]` chunk at `cursor`, advancing it
/// past the payload. `None` on a clean end **or** a truncated tail (a truncated
/// fixture must stop the replay, never panic mid-frame).
pub fn next_chunk<'a>(chunks: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let header_end = cursor.checked_add(4)?;
    let header = chunks.get(*cursor..header_end)?;
    let len = u32::from_le_bytes(header.try_into().expect("4-byte slice")) as usize;
    let payload_end = header_end.checked_add(len)?;
    let payload = chunks.get(header_end..payload_end)?;
    *cursor = payload_end;
    Some(payload)
}

/// Count the chunks in a stream and report the byte offset reached — the
/// fixture-integrity helper the tests assert with (`offset == chunks.len()`
/// means no trailing garbage).
pub fn walk_chunks(chunks: &[u8]) -> (usize, usize) {
    let mut cursor = 0usize;
    let mut count = 0usize;
    while next_chunk(chunks, &mut cursor).is_some() {
        count += 1;
    }
    (count, cursor)
}

// ---------------------------------------------------------------------------
// TerminalFeed — the emulator plus the replay cursor
// ---------------------------------------------------------------------------

/// The emulator and its replay position: everything that mutates **off** the
/// signal path. [`run_feed`] drives it; the grid widget reads its screen.
pub struct TerminalFeed {
    profile: Profile,
    parser: vt100::Parser,
    chunks: &'static [u8],
    /// Byte offset of the next unread chunk header.
    cursor: usize,
    /// Index of the next chunk (drives the `i * (1000 / hz)` schedule).
    next_index: usize,
    /// Diagnostics: chunks and bytes actually handed to the emulator.
    fed_chunks: usize,
    fed_bytes: usize,
}

impl TerminalFeed {
    /// A feed positioned at chunk 0 with a blank [`ROWS`]x[`COLS`] screen and no
    /// scrollback (the fixtures never scroll back, and a scrollback buffer would
    /// only add allocation churn).
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            parser: vt100::Parser::new(ROWS, COLS, 0),
            chunks: profile.chunks(),
            cursor: 0,
            next_index: 0,
            fed_chunks: 0,
            fed_bytes: 0,
        }
    }

    /// The emulator's current visible screen.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Feed **every** chunk whose scheduled time (`i * 1000 / hz` ms) has passed,
    /// returning how many were fed. This is the "coalesce the repaint, never drop
    /// input bytes" half of the mechanism: a late wake catches up on all due
    /// chunks in one pass rather than skipping any.
    pub fn feed_due(&mut self, elapsed: Duration) -> usize {
        let Some(interval_ms) = self.profile.interval_ms() else {
            return 0;
        };
        let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
        let total = self.profile.chunk_count();
        let mut fed = 0usize;
        while self.next_index < total {
            if self.next_index as f64 * interval_ms > elapsed_ms {
                break;
            }
            match next_chunk(self.chunks, &mut self.cursor) {
                Some(bytes) => {
                    self.parser.process(bytes);
                    self.fed_bytes += bytes.len();
                    self.fed_chunks += 1;
                }
                // Truncated fixture: stop cleanly instead of spinning.
                None => {
                    self.next_index = total;
                    break;
                }
            }
            self.next_index += 1;
            fed += 1;
        }
        fed
    }

    /// Whether every chunk has been fed — the feed loop's exit condition (and
    /// immediately true for [`Profile::Idle`]).
    pub fn is_drained(&self) -> bool {
        self.next_index >= self.profile.chunk_count()
    }

    /// Chunks fed so far, for the on-page readout.
    pub fn fed_chunks(&self) -> usize {
        self.fed_chunks
    }

    /// Bytes fed so far, for the on-page readout.
    pub fn fed_bytes(&self) -> usize {
        self.fed_bytes
    }
}

// ---------------------------------------------------------------------------
// Style key and run batching (pure)
// ---------------------------------------------------------------------------

/// Everything about a cell that affects how it paints. Two consecutive cells
/// batch into one run iff their `CellStyle`s are equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CellStyle {
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    dim: bool,
}

impl CellStyle {
    fn from_cell(cell: &vt100::Cell) -> Self {
        Self {
            fg: cell.fgcolor(),
            bg: cell.bgcolor(),
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
            dim: cell.dim(),
        }
    }

    /// Resolved `(foreground, background)`, applying `inverse` (swap) and `dim`
    /// (alpha). The background is `None` when it resolves to [`DEFAULT_BG`],
    /// which the whole-grid backdrop already covers — one fewer rect per run.
    ///
    /// The two default sentinels are resolved against their own fallbacks
    /// **before** `inverse` swaps them: swapping first would resolve both
    /// `Color::Default`s by slot and leave `ESC[7m` on an otherwise-unstyled cell
    /// painting light-on-dark, i.e. not inverted at all.
    fn resolve(self) -> (Color, Option<Color>) {
        let fg = resolve_color(self.fg, DEFAULT_FG);
        let bg = resolve_color(self.bg, DEFAULT_BG);
        let (mut fg, bg) = if self.inverse { (bg, fg) } else { (fg, bg) };
        if self.dim {
            fg = fg.with_alpha(DIM_ALPHA);
        }
        (fg, (bg != DEFAULT_BG).then_some(bg))
    }

    /// The shaping style for this run's text at the derived `font_size` (see the
    /// module docs' width-derived cell geometry).
    fn text_style(self, font_size: f32, color: Color) -> TextStyle {
        TextStyle {
            family: FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
            // The bundled IBM Plex Mono ships Regular/Medium/SemiBold/Italic;
            // 600 is its heaviest real face, so asking for it avoids parley
            // synthesizing a bold from Regular.
            weight: if self.bold {
                FontWeight::SEMI_BOLD
            } else {
                FontWeight::REGULAR
            },
            style: if self.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            line_height: LineHeight::FontSizeRelative(LINE_HEIGHT),
            ..TextStyle::new(font_size, color)
        }
    }
}

/// Map a `vt100::Color` onto a paint colour: the default sentinel onto
/// `fallback`, `Idx` through the ANSI 16 / xterm-256 tables, `Rgb` straight
/// through.
fn resolve_color(color: vt100::Color, fallback: Color) -> Color {
    match color {
        vt100::Color::Default => fallback,
        vt100::Color::Idx(i) => indexed_color(i),
        vt100::Color::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
    }
}

/// The xterm 256-colour table: 0-15 the ANSI palette, 16-231 a 6x6x6 cube,
/// 232-255 a 24-step greyscale ramp. The fixture subset only ever emits 0-7, but
/// a wrong-looking fallback for the rest would be a silent divergence from every
/// other terminal rather than a visible one.
fn indexed_color(i: u8) -> Color {
    match i {
        0..=15 => ANSI_16[i as usize],
        16..=231 => {
            const LEVELS: [u8; 6] = [0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF];
            let v = i - 16;
            Color::from_rgb8(
                LEVELS[(v / 36) as usize],
                LEVELS[((v % 36) / 6) as usize],
                LEVELS[(v % 6) as usize],
            )
        }
        232..=255 => {
            let g = 8 + (i - 232) * 10;
            Color::from_rgb8(g, g, g)
        }
    }
}

/// One batched run: consecutive same-style cells on one row, shaped once.
pub struct GridRun {
    row: u16,
    col: u16,
    /// Cells covered — the background rect's width in cells.
    cells: u16,
    style: CellStyle,
    text: String,
    /// Filled by the shaping pass in [`TerminalGridWidget::layout`].
    layout: Option<TextLayout>,
}

impl GridRun {
    fn blank() -> Self {
        Self {
            row: 0,
            col: 0,
            cells: 0,
            style: CellStyle::default(),
            text: String::new(),
            layout: None,
        }
    }

    /// The run's starting column (test/diagnostic accessor).
    pub fn col(&self) -> u16 {
        self.col
    }

    /// The run's row (test/diagnostic accessor).
    pub fn row(&self) -> u16 {
        self.row
    }

    /// The run's text (test/diagnostic accessor).
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// A reusable batch of [`GridRun`]s: `runs[..len]` are live, `runs[len..]` are
/// retained empties whose `String` allocations get reused next generation.
/// Rebuilding the batch every generation must not re-allocate 1800 `String`s.
#[derive(Default)]
pub struct RunBatch {
    runs: Vec<GridRun>,
    len: usize,
}

impl RunBatch {
    /// The live runs.
    pub fn active(&self) -> &[GridRun] {
        &self.runs[..self.len]
    }

    /// The live runs, mutably — the shaping pass fills each run's `layout`.
    fn active_mut(&mut self) -> &mut [GridRun] {
        &mut self.runs[..self.len]
    }

    /// How many runs the current generation batched to.
    pub fn count(&self) -> usize {
        self.len
    }

    /// Drop every live run (keeping their allocations) and start a fresh batch.
    fn begin(&mut self) {
        self.len = 0;
    }

    /// Start a run at `(row, col)` in the slot after the committed ones.
    fn open(&mut self, row: u16, col: u16, style: CellStyle) {
        if self.runs.len() == self.len {
            self.runs.push(GridRun::blank());
        }
        let slot = &mut self.runs[self.len];
        slot.row = row;
        slot.col = col;
        slot.cells = 0;
        slot.style = style;
        slot.text.clear();
        slot.layout = None;
    }

    /// Append one cell's glyph(s) to the open run, advancing its column span by
    /// `span` (2 for a wide character, whose continuation cell contributes no
    /// glyph of its own — see [`batch_screen`]).
    fn push_cell(&mut self, glyph: &str, span: u16) {
        let slot = &mut self.runs[self.len];
        slot.text.push_str(glyph);
        slot.cells += span;
    }

    /// Commit the open run, or discard it if nothing would paint.
    ///
    /// A run over the default background contributes no pixels for its trailing
    /// blanks, so those are trimmed; a run that is then empty (a blank stretch on
    /// the default background — most of a terminal screen most of the time) is
    /// discarded, and its slot is reused by the next `open`. A run with a
    /// non-default background always commits: its rect paints even where its text
    /// does not.
    fn commit(&mut self) {
        let slot = &mut self.runs[self.len];
        if slot.cells == 0 {
            return;
        }
        let default_bg = slot.style.resolve().1.is_none();
        if default_bg {
            let trimmed = slot.text.trim_end_matches(' ').len();
            slot.text.truncate(trimmed);
            if slot.text.is_empty() {
                return;
            }
        }
        self.len += 1;
    }
}

/// Batch one emulator screen into same-style runs.
///
/// Walks each row left to right, coalescing consecutive cells whose whole
/// [`CellStyle`] matches into one run. A cell with no contents contributes a
/// single space so the following glyphs stay on their columns; the second half of
/// a wide character contributes nothing of its own (its glyph belongs to the
/// first half, which spans two columns instead of one). The fixture subset is
/// pure ASCII, so the wide path is correctness-for-free rather than something
/// these profiles exercise.
pub fn batch_screen(screen: &vt100::Screen, out: &mut RunBatch) {
    out.begin();
    for row in 0..ROWS {
        let mut open: Option<CellStyle> = None;
        for col in 0..COLS {
            let cell = screen.cell(row, col);
            if cell.is_some_and(vt100::Cell::is_wide_continuation) {
                continue;
            }
            let style = cell.map(CellStyle::from_cell).unwrap_or_default();
            let glyph = match cell {
                Some(c) if c.has_contents() => c.contents(),
                _ => " ",
            };
            let span = if cell.is_some_and(vt100::Cell::is_wide) {
                2
            } else {
                1
            };
            if open != Some(style) {
                if open.is_some() {
                    out.commit();
                }
                out.open(row, col, style);
                open = Some(style);
            }
            out.push_cell(glyph, span);
        }
        if open.is_some() {
            out.commit();
        }
    }
}

// ---------------------------------------------------------------------------
// The feed loop — the coalescing wake source
// ---------------------------------------------------------------------------

/// The UI-thread feed loop: wake every [`REPAINT_INTERVAL`], feed every due
/// chunk, publish **one** generation, repeat until the stream drains, the page
/// tears down, or a newer loop supersedes this one.
///
/// `epoch` is the page's "which loop is live" counter: every (re)start bumps it,
/// so an older loop sees its own `my_epoch` go stale and returns instead of
/// racing the new one on the same signal. `cancelled` is the teardown flag the
/// component's `on_cleanup` sets (`Arc<AtomicBool>` rather than the `Rc<Cell<_>>`
/// used for the epoch, because `on_cleanup` takes a `Send + Sync` closure — both
/// ends still live on the UI thread).
///
/// Every `await` point releases the [`RefCell`] borrow first — a borrow held
/// across a suspension would panic the moment the grid widget laid out.
async fn run_feed(
    feed: Rc<RefCell<TerminalFeed>>,
    generation: RwSignal<u64>,
    epoch: Rc<Cell<u64>>,
    my_epoch: u64,
    cancelled: Arc<AtomicBool>,
) {
    let start = Instant::now();
    let mut tick = 0u32;
    loop {
        if cancelled.load(Ordering::Relaxed) || epoch.get() != my_epoch {
            return;
        }
        let fed = feed.borrow_mut().feed_due(start.elapsed());
        if fed > 0 {
            // ONE signal write per wake, never one per chunk: this is the whole
            // point of the mechanism (see the module docs).
            generation.set(generation.get_untracked() + 1);
        }
        if feed.borrow().is_drained() {
            return;
        }
        // Sleep to the next absolute grid point rather than "now + interval", so
        // a slow frame can't drift the replay schedule.
        tick += 1;
        let deadline = REPAINT_INTERVAL * tick;
        let elapsed = start.elapsed();
        if let Some(nap) = deadline.checked_sub(elapsed) {
            let _ = spawn_blocking(move || std::thread::sleep(nap)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Page + Component
// ---------------------------------------------------------------------------

/// See the page-fn contract in [`crate::pages`]. The Terminal section reads no
/// shared signal — it only mounts [`TerminalPage`]'s own retained state.
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(TerminalPage)
}

/// The Terminal section's own [`Component`]: owns the emulator, the generation
/// signal, and the feed loop.
struct TerminalPage;

/// Live grid diagnostics the widget publishes for the page's readout.
///
/// A plain `Cell<GridStats>` rather than a signal: the widget writes it during
/// layout/paint, and the readout is re-read by the very rebuild the generation
/// signal already triggers — writing a *second* signal from paint would be a
/// wake loop, not an improvement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridStats {
    /// Runs batched for the last shaped generation.
    pub runs: usize,
    /// Largest run count seen since the replay (re)started.
    pub max_runs: usize,
    /// Frames the grid painted since the replay (re)started.
    pub painted_frames: u64,
    /// The solved font size, in logical px.
    pub font_size: f32,
    /// The solved cell box, in logical px.
    pub cell_width: f64,
    pub cell_height: f64,
}

/// [`TerminalPage`]'s retained state.
struct TerminalPageState {
    /// The selected profile — what a Play tap replays.
    profile: Profile,
    /// Whether a feed loop is live (a stop, a drain, or teardown clears it; a
    /// drained replay leaves this set until the next tap, which is honest —
    /// the loop returned on its own).
    running: bool,
    /// Bumped once per coalescing tick that fed anything, and once per control
    /// tap — the **only** signal this page writes.
    generation: RwSignal<u64>,
    /// Shared with the feed loop: written there, read by the grid widget's
    /// layout. `Rc<RefCell<_>>` rather than a signal precisely so a chunk feed is
    /// not a signal write.
    feed: Rc<RefCell<TerminalFeed>>,
    /// Which feed loop is live — see [`run_feed`].
    epoch: Rc<Cell<u64>>,
    /// Set by this component's `on_cleanup`; stops the live loop at its next
    /// wake.
    cancelled: Arc<AtomicBool>,
    /// The grid's live diagnostics — see [`GridStats`].
    stats: Rc<Cell<GridStats>>,
}

impl Component for TerminalPage {
    type State = TerminalPageState;

    fn init(&self) -> TerminalPageState {
        // Nothing replays until the user taps Play: the page must cost zero
        // frames at rest like every other section (and a 120 Hz firehose
        // starting itself on a section switch would be a surprise, not a demo).
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let cancelled = Arc::clone(&cancelled);
            // `on_cleanup`, never a `Drop` impl — `docs/CODE_STANDARDS.md`'s
            // State & Reactivity conventions.
            on_cleanup(move || cancelled.store(true, Ordering::Relaxed));
        }
        let profile = Profile::BuildLog;
        TerminalPageState {
            profile,
            running: false,
            generation: RwSignal::new(0),
            feed: Rc::new(RefCell::new(TerminalFeed::new(profile))),
            epoch: Rc::new(Cell::new(0)),
            cancelled,
            stats: Rc::new(Cell::new(GridStats::default())),
        }
    }

    fn build(&self, state: &mut TerminalPageState) -> impl View<TerminalPageState> {
        // Tracked read: the feed loop's generation write wakes this rebuild, and
        // nothing else does.
        let generation = state.generation.get();
        let stats = state.stats.get();
        let (fed_chunks, fed_bytes, drained) = {
            let feed = state.feed.borrow();
            (feed.fed_chunks(), feed.fed_bytes(), feed.is_drained())
        };

        let mut children: Vec<FlexChild<TerminalPageState>> = vec![
            block(vec![
                inflexible(label("Terminal")),
                gap(6.0),
                inflexible(caption(
                    "A checked-in byte stream replayed through a real VT emulator (the vt100 \
                     crate) into an 80x45 grid. Pick a profile, then Play \u{2014} the replay \
                     coalesces its repaints to 30 Hz and writes one signal per tick, never one \
                     per chunk.",
                )),
            ]),
            profile_block(state),
            transport_block(state, drained),
            readout_block(state.profile, stats, fed_chunks, fed_bytes, drained),
        ];

        children.push(gap(8.0));
        children.push(inflexible(TerminalGridView {
            generation,
            feed: Rc::clone(&state.feed),
            stats: Rc::clone(&state.stats),
        }));
        children.push(gap(16.0));

        any(Padding(
            EdgeInsets::all(16.0),
            FlexView::new(Axis::Vertical, children),
        ))
    }
}

/// (Re)start the replay on `profile`: a fresh emulator, a fresh schedule, a
/// bumped epoch (which retires any loop still running), and one signal write so
/// the grid swaps to the new feed on the next rebuild.
fn start_replay(state: &mut TerminalPageState, profile: Profile) {
    state.profile = profile;
    state.feed = Rc::new(RefCell::new(TerminalFeed::new(profile)));
    state.stats.set(GridStats::default());
    state.running = true;

    let my_epoch = state.epoch.get() + 1;
    state.epoch.set(my_epoch);
    state.generation.set(state.generation.get_untracked() + 1);

    spawn_local(run_feed(
        Rc::clone(&state.feed),
        state.generation,
        Rc::clone(&state.epoch),
        my_epoch,
        Arc::clone(&state.cancelled),
    ));
}

/// Retire the live feed loop (it returns at its next wake) and leave the screen
/// exactly where it stopped.
fn stop_replay(state: &mut TerminalPageState) {
    state.epoch.set(state.epoch.get() + 1);
    state.running = false;
    state.generation.set(state.generation.get_untracked() + 1);
}

/// The profile picker: one button per fixture, the selected one in the primary
/// role. Tapping while a replay is running restarts it on the new profile
/// (there is no mid-replay profile switch — the emulator state belongs to the
/// profile that produced it).
fn profile_block(state: &TerminalPageState) -> FlexChild<TerminalPageState> {
    let selected = state.profile;
    let buttons: Vec<FlexChild<TerminalPageState>> = Profile::ALL
        .into_iter()
        .flat_map(|profile| {
            let style = if profile == selected {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            };
            [
                inflexible(
                    button(profile.name(), move |state: &mut TerminalPageState| {
                        if state.running {
                            start_replay(state, profile);
                        } else {
                            state.profile = profile;
                            state.feed = Rc::new(RefCell::new(TerminalFeed::new(profile)));
                            state.stats.set(GridStats::default());
                            state.generation.set(state.generation.get_untracked() + 1);
                        }
                    })
                    .style(style)
                    .small(),
                ),
                gap(4.0),
            ]
        })
        .collect();

    block(vec![
        inflexible(label("Profile")),
        gap(6.0),
        inflexible(FlexView::new(Axis::Vertical, buttons)),
        gap(2.0),
        inflexible(caption(selected.blurb())),
    ])
}

/// Play / Restart / Stop.
fn transport_block(state: &TerminalPageState, drained: bool) -> FlexChild<TerminalPageState> {
    let play_label = if state.running && !drained {
        "Restart"
    } else {
        "Play"
    };
    let profile = state.profile;

    let mut rows = vec![
        inflexible(label("Transport")),
        gap(6.0),
        inflexible(
            button(play_label, move |state: &mut TerminalPageState| {
                start_replay(state, profile)
            })
            .style(ButtonStyle::Primary)
            .small(),
        ),
        gap(6.0),
    ];
    if state.running {
        rows.push(inflexible(
            button("Stop", stop_replay)
                .style(ButtonStyle::Secondary)
                .small(),
        ));
        rows.push(gap(6.0));
    }
    rows.push(inflexible(caption(
        "A stopped (or drained) replay writes no signal and requests no frame \u{2014} the grid \
         costs nothing at rest.",
    )));
    block(rows)
}

/// The live diagnostics readout: what the emulator has consumed, what the grid
/// batched it into, and the geometry it solved.
fn readout_block(
    profile: Profile,
    stats: GridStats,
    fed_chunks: usize,
    fed_bytes: usize,
    drained: bool,
) -> FlexChild<TerminalPageState> {
    let progress = format!(
        "{}: {fed_chunks}/{} chunks \u{b7} {fed_bytes} bytes fed{}",
        profile.name(),
        profile.chunk_count(),
        if drained { " \u{b7} drained" } else { "" },
    );
    let runs = format!(
        "runs/frame: {} (peak {}) \u{b7} painted frames: {}",
        stats.runs, stats.max_runs, stats.painted_frames,
    );
    let geometry = format!(
        "geometry: {COLS}x{ROWS} cells \u{b7} font {:.2}px \u{b7} cell {:.2}x{:.2}px",
        stats.font_size, stats.cell_width, stats.cell_height,
    );

    block(vec![
        inflexible(label("Live")),
        gap(6.0),
        inflexible(mono_line(progress)),
        inflexible(mono_line(runs)),
        inflexible(mono_line(geometry)),
    ])
}

// ---------------------------------------------------------------------------
// Section chrome (mirrors `camera.rs`'s identical helpers — duplicated
// per-module by established convention, not shared)
// ---------------------------------------------------------------------------

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

/// A demo heading in the accent role.
fn label(s: impl Into<String>) -> impl View<TerminalPageState> {
    text(s).size(13.0).color(accent())
}

/// A muted per-demo caption.
fn caption(s: impl Into<String>) -> impl View<TerminalPageState> {
    text(s).size(11.0).color(muted())
}

/// One monospace diagnostic line, so the readout's numbers stay column-aligned
/// as they change.
fn mono_line(s: impl Into<String>) -> impl View<TerminalPageState> {
    text(s)
        .size(11.0)
        .color(muted())
        .family(FontFamily::stack_with_generic(
            ["IBM Plex Mono"],
            GenericSlot::Monospace,
        ))
}

/// A fixed-height vertical spacer between demo blocks.
fn gap(h: f64) -> FlexChild<TerminalPageState> {
    inflexible(SizedBox(None, Some(h)))
}

/// Wrap a demo's rows in a padded vertical column (one showcase block).
fn block(children: Vec<FlexChild<TerminalPageState>>) -> FlexChild<TerminalPageState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

// ---------------------------------------------------------------------------
// TerminalGridView / TerminalGridWidget
// ---------------------------------------------------------------------------

/// The declarative grid. See [`TerminalGridWidget`].
pub struct TerminalGridView {
    generation: u64,
    feed: Rc<RefCell<TerminalFeed>>,
    stats: Rc<Cell<GridStats>>,
}

/// The cell box the grid paints on, and the font size it was derived from.
///
/// Derived from the available content width rather than fixed, so all [`COLS`]
/// columns stay visible on a phone — see the module docs' width-derived cell
/// geometry section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellGeometry {
    /// The solved font size in logical px.
    pub font_size: f32,
    /// Measured monospace advance (one cell's width) in logical px.
    pub width: f64,
    /// Measured line box height (one cell's height) in logical px.
    pub height: f64,
}

impl CellGeometry {
    /// The whole grid's painted width: [`COLS`] cells.
    pub fn grid_width(self) -> f64 {
        self.width * f64::from(COLS)
    }

    /// The whole grid's painted height: [`ROWS`] cells.
    pub fn grid_height(self) -> f64 {
        self.height * f64::from(ROWS)
    }
}

/// Solve the cell box that fits exactly [`COLS`]x[`ROWS`] cells into `available`,
/// from a `measure(font_size) -> (cell_advance, line_height)` probe of the text
/// engine's own monospace face.
///
/// Pure (the measurement is injected) so the arithmetic is unit-testable without
/// a font, a `TextContext`, or a window:
///
/// 1. Measure once at [`PROBE_FONT_SIZE`] to get the face's advance and
///    line-height **ratios** — never assume 0.6em, measure it.
/// 2. Start from the size those ratios say fits ([`COLS`] columns of width,
///    [`ROWS`] rows of height, whichever binds first).
/// 3. Re-measure at that size and shrink if the *real* metrics overflow, up to
///    [`FIT_ITERATIONS`] times, so the returned box fits `available` to within
///    [`FIT_TOLERANCE`] — which is what turns "no clipped column" from an
///    assumption about the shaper into a property of the result.
///
/// An unbounded axis simply does not constrain; with both unbounded the result is
/// [`FALLBACK_FONT_SIZE`]. The size is floored at [`MIN_FONT_SIZE`], the one case
/// where the returned box may genuinely overflow (see that constant).
pub fn solve_cell_geometry(
    available: Size,
    mut measure: impl FnMut(f32) -> (f64, f64),
) -> CellGeometry {
    let (probe_width, probe_height) = measure(PROBE_FONT_SIZE);
    let width_ratio = probe_width / f64::from(PROBE_FONT_SIZE);
    let height_ratio = probe_height / f64::from(PROBE_FONT_SIZE);

    let mut font_size = start_font_size(available, width_ratio, height_ratio);
    let mut metrics = measure(font_size);
    for pass in 0..FIT_ITERATIONS {
        let shrink = fit_factor(available, metrics);
        if shrink >= 1.0 {
            break;
        }
        // The first pass trusts the measured ratio (exact for a linear face);
        // later passes back off geometrically — see [`FIT_BACKOFF`].
        let shrink = if pass == 0 {
            shrink
        } else {
            shrink.min(FIT_BACKOFF)
        };
        let next = (f64::from(font_size) * shrink) as f32;
        // A shrink that can't make progress (already at the floor, or rounded
        // back to the same f32) must not spin.
        if next >= font_size || next < MIN_FONT_SIZE || next.is_nan() {
            break;
        }
        font_size = next;
        metrics = measure(font_size);
    }

    CellGeometry {
        font_size,
        width: metrics.0,
        height: metrics.1,
    }
}

/// Step 2 of [`solve_cell_geometry`]: the size the measured ratios predict will
/// fit, floored at [`MIN_FONT_SIZE`], or [`FALLBACK_FONT_SIZE`] when neither axis
/// is bounded.
fn start_font_size(available: Size, width_ratio: f64, height_ratio: f64) -> f32 {
    let mut size = f64::INFINITY;
    if available.width.is_finite() && available.width > 0.0 && width_ratio > 0.0 {
        size = size.min(available.width / (f64::from(COLS) * width_ratio));
    }
    if available.height.is_finite() && available.height > 0.0 && height_ratio > 0.0 {
        size = size.min(available.height / (f64::from(ROWS) * height_ratio));
    }
    if size.is_finite() {
        (size as f32).max(MIN_FONT_SIZE)
    } else {
        FALLBACK_FONT_SIZE
    }
}

/// Step 3 of [`solve_cell_geometry`]: how much the current cell box has to shrink
/// to fit `available` (`>= 1.0` means it already fits, within [`FIT_TOLERANCE`]).
/// An unbounded axis never constrains.
fn fit_factor(available: Size, (cell_width, cell_height): (f64, f64)) -> f64 {
    let mut factor = f64::INFINITY;
    if available.width.is_finite() && cell_width > 0.0 {
        factor = factor.min((available.width + FIT_TOLERANCE) / (f64::from(COLS) * cell_width));
    }
    if available.height.is_finite() && cell_height > 0.0 {
        factor = factor.min((available.height + FIT_TOLERANCE) / (f64::from(ROWS) * cell_height));
    }
    if factor.is_finite() { factor } else { 1.0 }
}

/// The retained grid: batches + shapes the emulator screen on every new
/// generation, then paints one background rect and one glyph run per batched run.
///
/// Shaping happens in `layout` because that is the only pass with a
/// `TextContext`, which is why a new generation must report
/// `ChangeFlags::LAYOUT` — a bare `PAINT` would leave the grid frozen under
/// Android's intra-frame layout-skip gate.
pub struct TerminalGridWidget {
    generation: u64,
    feed: Rc<RefCell<TerminalFeed>>,
    stats: Rc<Cell<GridStats>>,
    batch: RunBatch,
    /// Which generation `batch` was shaped from (`None` = never).
    shaped: Option<u64>,
    /// The content box the geometry was solved against, and the solution.
    /// Re-solved only when that box changes (a rotation, a density change), and a
    /// re-solve invalidates `shaped` — the runs carry the old font size.
    geometry: Option<(Size, CellGeometry)>,
    /// Peak run count and painted-frame count since this widget was built, for
    /// the page readout.
    max_runs: usize,
    painted_frames: u64,
}

impl<State: 'static> View<State> for TerminalGridView {
    type Element = TerminalGridWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TerminalGridWidget {
        TerminalGridWidget {
            generation: self.generation,
            feed: Rc::clone(&self.feed),
            stats: Rc::clone(&self.stats),
            batch: RunBatch::default(),
            shaped: None,
            geometry: None,
            max_runs: 0,
            painted_frames: 0,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TerminalGridWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // A (re)start swaps in a whole new feed; the generation alone cannot say
        // so, since a restart also resets what the emulator holds.
        let same_feed = Rc::ptr_eq(&prev.feed, &self.feed);
        if same_feed && prev.generation == self.generation {
            return ChangeFlags::NONE;
        }
        element.generation = self.generation;
        if !same_feed {
            element.feed = Rc::clone(&self.feed);
            element.max_runs = 0;
            element.painted_frames = 0;
        }
        // LAYOUT, not just PAINT: the reshape happens in `layout`.
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

impl Widget for TerminalGridWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The cell box is derived from the content box, not fixed (see the module
        // docs). Re-solved only when that box changes; a change re-shapes, since
        // every run carries the old size.
        let available = bc.max();
        let geometry = match self.geometry {
            Some((solved_for, geometry)) if solved_for == available => geometry,
            _ => {
                let geometry =
                    solve_cell_geometry(available, |font_size| measure_cell(ctx, font_size));
                self.geometry = Some((available, geometry));
                self.shaped = None;
                geometry
            }
        };

        if self.shaped != Some(self.generation) {
            self.reshape(ctx, geometry.font_size);
            self.shaped = Some(self.generation);
        }

        // Exactly COLS x ROWS cells: the constraint that keeps the grid from
        // filling leftover height with extra rows.
        bc.constrain(Size::new(geometry.grid_width(), geometry.grid_height()))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let metrics = self.geometry.map(|(_, g)| g).unwrap_or(CellGeometry {
            font_size: FALLBACK_FONT_SIZE,
            width: f64::from(FALLBACK_FONT_SIZE),
            height: f64::from(FALLBACK_FONT_SIZE * LINE_HEIGHT),
        });
        let origin = ctx.origin();
        let size = ctx.size();

        // The derived geometry already fits the content box, so nothing should
        // reach this clip — it stays as the backstop that makes "nothing outside
        // the COLS x ROWS grid can paint" true by construction.
        scene.push_clip(origin, size);
        scene.fill_rect(origin, size, DEFAULT_BG);

        for run in self.batch.active() {
            let x = origin.x + f64::from(run.col) * metrics.width;
            let y = origin.y + f64::from(run.row) * metrics.height;
            let (_, bg) = run.style.resolve();
            if let Some(bg) = bg {
                scene.fill_rect(
                    Point::new(x, y),
                    Size::new(f64::from(run.cells) * metrics.width, metrics.height),
                    bg,
                );
            }
            if let Some(layout) = &run.layout {
                for glyphs in layout.to_scene_runs(Point::new(x, y)) {
                    scene.draw_glyph_run(glyphs);
                }
            }
            if run.style.underline {
                let uy = y + metrics.height * UNDERLINE_Y_FRACTION;
                let (fg, _) = run.style.resolve();
                scene.stroke_line(
                    Point::new(x, uy),
                    Point::new(x + f64::from(run.cells) * metrics.width, uy),
                    UNDERLINE_WIDTH,
                    fg,
                );
            }
        }

        scene.pop_clip();

        self.painted_frames += 1;
        self.max_runs = self.max_runs.max(self.batch.count());
        self.stats.set(GridStats {
            runs: self.batch.count(),
            max_runs: self.max_runs,
            painted_frames: self.painted_frames,
            font_size: metrics.font_size,
            cell_width: metrics.width,
            cell_height: metrics.height,
        });
        // No `request_frame`: the feed loop's generation write is the sole wake
        // source, which is what lets a stopped/idle replay settle at zero frames.
    }
}

impl TerminalGridWidget {
    /// Batch the emulator's current screen and shape every run.
    ///
    /// The `Rc` is cloned out first so the `RefCell` borrow does not also borrow
    /// `self` (the batch lives in `self`).
    fn reshape(&mut self, ctx: &mut LayoutCtx, font_size: f32) {
        let feed = Rc::clone(&self.feed);
        {
            let feed = feed.borrow();
            batch_screen(feed.screen(), &mut self.batch);
        }
        // One `TextContext::layout` per run — one vello `draw_glyphs` per run
        // downstream. `None` max_width: a run never wraps.
        let text_ctx = ctx.text_context::<TextContext>();
        for run in self.batch.active_mut() {
            let (fg, _) = run.style.resolve();
            let style = run.style.text_style(font_size, fg);
            run.layout = Some(text_ctx.layout(&run.text, &style, None));
        }
    }
}

/// Measure the monospace cell box at `font_size`: shape [`CELL_PROBE`] and divide
/// its advance by its length (its height is the shaped line box). The probe
/// [`solve_cell_geometry`] solves against — never an assumed advance ratio.
fn measure_cell(ctx: &mut LayoutCtx, font_size: f32) -> (f64, f64) {
    let style = CellStyle::default().text_style(font_size, DEFAULT_FG);
    let text_ctx = ctx.text_context::<TextContext>();
    let probe = text_ctx.layout(CELL_PROBE, &style, None);
    let size = probe.size();
    let n = CELL_PROBE.chars().count() as f64;
    let width = if size.width > 0.0 && n > 0.0 {
        size.width / n
    } else {
        f64::from(font_size)
    };
    let height = if size.height > 0.0 {
        size.height
    } else {
        f64::from(font_size * LINE_HEIGHT)
    };
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_reactive::ReactiveRuntime;
    use frust_text::TextContext as RawTextContext;
    use kurbo::BezPath;
    use peniko::Brush;
    use reactive_graph::owner::Owner;
    use std::any::Any;

    // -----------------------------------------------------------------
    // Fixture integrity
    // -----------------------------------------------------------------

    #[test]
    fn every_fixture_walks_to_its_manifest_chunk_count_with_no_trailing_bytes() {
        for profile in Profile::ALL {
            let bytes = profile.chunks();
            let (count, offset) = walk_chunks(bytes);
            assert_eq!(
                count,
                profile.chunk_count(),
                "{}: chunk count must match its .json manifest",
                profile.name()
            );
            assert_eq!(
                offset,
                bytes.len(),
                "{}: the whole file must be length-prefixed chunks (no trailing bytes)",
                profile.name()
            );
        }
    }

    #[test]
    fn idle_fixture_is_empty_and_drains_immediately() {
        assert!(Profile::Idle.chunks().is_empty());
        let mut feed = TerminalFeed::new(Profile::Idle);
        assert!(feed.is_drained(), "no chunks means drained at t=0");
        assert_eq!(feed.feed_due(Duration::from_secs(30)), 0);
    }

    #[test]
    fn chunk_reader_stops_on_a_truncated_tail_instead_of_panicking() {
        // A 3-byte payload declared, 2 supplied.
        let bytes = [3u8, 0, 0, 0, b'a', b'b'];
        let mut cursor = 0usize;
        assert!(next_chunk(&bytes, &mut cursor).is_none());
        assert_eq!(cursor, 0, "a rejected read must not advance the cursor");

        // A truncated 3-byte header.
        let short = [1u8, 0, 0];
        let mut cursor = 0usize;
        assert!(next_chunk(&short, &mut cursor).is_none());
    }

    #[test]
    fn chunk_reader_walks_a_hand_built_stream() {
        let mut bytes = Vec::new();
        for payload in [b"a".as_slice(), b"".as_slice(), b"hello".as_slice()] {
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
        }
        let mut cursor = 0usize;
        assert_eq!(next_chunk(&bytes, &mut cursor), Some(b"a".as_slice()));
        assert_eq!(next_chunk(&bytes, &mut cursor), Some(b"".as_slice()));
        assert_eq!(next_chunk(&bytes, &mut cursor), Some(b"hello".as_slice()));
        assert_eq!(next_chunk(&bytes, &mut cursor), None);
        assert_eq!(cursor, bytes.len());
    }

    // -----------------------------------------------------------------
    // Replay schedule: coalesce the repaint, never drop a byte
    // -----------------------------------------------------------------

    #[test]
    fn feed_emits_chunk_i_at_i_over_hz_and_catches_up_on_a_late_wake() {
        let profile = Profile::BuildLog; // 30 Hz => 33.33 ms apart
        let mut feed = TerminalFeed::new(profile);

        // t=0 releases exactly chunk 0.
        assert_eq!(feed.feed_due(Duration::ZERO), 1);
        // Just before chunk 1's slot: nothing new.
        assert_eq!(feed.feed_due(Duration::from_micros(33_000)), 0);
        // Past it: exactly one more.
        assert_eq!(feed.feed_due(Duration::from_micros(34_000)), 1);
        // A ~1-second-late wake must catch up on every due chunk (at 30 Hz,
        // chunks 0..=30 are due by t=1010ms), never skip one. 1010 rather than a
        // clean multiple of the 33.333… ms interval so the assertion doesn't
        // hinge on which side of a boundary the float lands.
        assert_eq!(feed.feed_due(Duration::from_millis(1010)), 29);
        assert_eq!(feed.fed_chunks(), 31);

        // Draining the whole schedule consumes exactly `chunk_count` chunks.
        feed.feed_due(Duration::from_secs(60));
        assert!(feed.is_drained());
        assert_eq!(feed.fed_chunks(), profile.chunk_count());
    }

    #[test]
    fn every_profile_drains_to_its_full_chunk_count_and_byte_length() {
        for profile in Profile::ALL {
            let mut feed = TerminalFeed::new(profile);
            feed.feed_due(Duration::from_secs(60));
            assert!(feed.is_drained(), "{}", profile.name());
            assert_eq!(
                feed.fed_chunks(),
                profile.chunk_count(),
                "{}",
                profile.name()
            );
            // Payload bytes = file length minus one 4-byte header per chunk.
            assert_eq!(
                feed.fed_bytes(),
                profile.chunks().len() - 4 * profile.chunk_count(),
                "{}: every payload byte must reach the emulator",
                profile.name()
            );
        }
    }

    #[test]
    fn repaint_interval_matches_the_documented_cap() {
        // The one knob: `REPAINT_HZ`. Guard the derived period against drift.
        let expected = Duration::from_secs_f64(1.0 / REPAINT_HZ);
        assert!(
            REPAINT_INTERVAL.abs_diff(expected) < Duration::from_micros(1),
            "{REPAINT_INTERVAL:?}"
        );
    }

    // -----------------------------------------------------------------
    // Run batching
    // -----------------------------------------------------------------

    /// A parser fed `bytes`, batched into runs.
    fn batch_of(bytes: &[u8]) -> RunBatch {
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        parser.process(bytes);
        let mut batch = RunBatch::default();
        batch_screen(parser.screen(), &mut batch);
        batch
    }

    #[test]
    fn blank_screen_batches_to_zero_runs() {
        let batch = batch_of(b"");
        assert_eq!(
            batch.count(),
            0,
            "a blank default-background screen paints no runs"
        );
    }

    #[test]
    fn same_style_cells_coalesce_and_a_style_change_splits() {
        // Two SGR colours, three cells each: two runs, not six.
        let batch = batch_of(b"\x1b[31mAAA\x1b[32mBBB");
        let runs = batch.active();
        assert_eq!(runs.len(), 2, "one run per style, not one per cell");
        assert_eq!(runs[0].text(), "AAA");
        assert_eq!(runs[0].col(), 0);
        assert_eq!(runs[1].text(), "BBB");
        assert_eq!(runs[1].col(), 3);
        assert_eq!(runs[0].row(), 0);
    }

    #[test]
    fn bold_splits_a_run_even_at_an_unchanged_colour() {
        let batch = batch_of(b"ab\x1b[1mcd\x1b[0mef");
        let runs = batch.active();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text(), "ab");
        assert_eq!(runs[1].text(), "cd");
        assert_eq!(runs[2].text(), "ef");
        assert!(runs[1].style.bold);
        assert!(!runs[0].style.bold);
    }

    #[test]
    fn interior_blanks_stay_inside_a_run_and_trailing_blanks_are_trimmed() {
        let batch = batch_of(b"ab   cd");
        let runs = batch.active();
        // One default-style run: the interior spaces must NOT split it (that
        // would turn every word into its own draw call), and the blank tail out
        // to column 79 must not be shaped.
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text(), "ab   cd");
    }

    #[test]
    fn a_non_default_background_run_survives_even_when_blank() {
        // A coloured background paints even with no glyphs, so a blank run over
        // one must not be discarded.
        let batch = batch_of(b"\x1b[41m   \x1b[0m");
        let runs = batch.active();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text(), "   ");
        assert!(runs[0].style.resolve().1.is_some(), "has a background rect");
    }

    #[test]
    fn per_row_batching_keeps_runs_on_their_own_rows() {
        let batch = batch_of(b"aa\r\nbb");
        let runs = batch.active();
        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].row(), runs[0].text()), (0, "aa"));
        assert_eq!((runs[1].row(), runs[1].text()), (1, "bb"));
    }

    #[test]
    fn batch_reuses_its_run_allocations_across_generations() {
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        let mut batch = RunBatch::default();
        parser.process(b"\x1b[31mAAA\x1b[32mBBB");
        batch_screen(parser.screen(), &mut batch);
        assert_eq!(batch.count(), 2);
        // The slot vec also holds the uncommitted scratch slot each row's blank
        // tail opened and discarded, so capacity >= committed count.
        let capacity = batch.runs.len();
        assert!(capacity >= batch.count());

        // A screen that batches to fewer runs must keep every slot for reuse —
        // re-allocating ~1800 `String`s per generation is exactly what this
        // structure exists to avoid.
        parser.process(b"\x1b[H\x1b[0maaaaaa");
        batch_screen(parser.screen(), &mut batch);
        assert_eq!(batch.count(), 1);
        assert_eq!(
            batch.runs.len(),
            capacity,
            "slots are retained, not dropped"
        );
    }

    // -----------------------------------------------------------------
    // Run counts per profile — what the readout reports
    // -----------------------------------------------------------------

    /// Batch the screen after replaying `chunks` worth of `profile`.
    fn run_count_after(profile: Profile, chunks: usize) -> usize {
        let mut feed = TerminalFeed::new(profile);
        let interval = profile.interval_ms().unwrap_or(0.0);
        feed.feed_due(Duration::from_secs_f64(
            (chunks as f64 * interval) / 1000.0 + 0.001,
        ));
        let mut batch = RunBatch::default();
        batch_screen(feed.screen(), &mut batch);
        batch.count()
    }

    #[test]
    fn htop_batches_to_the_density_band_the_fixture_readme_predicts() {
        // 44 visible styled rows x 40 two-cell colour runs, minus the ~1/64 of
        // adjacent pairs that coincidentally draw the same fg+bg and merge.
        let runs = run_count_after(Profile::Htop, 3);
        assert!(
            (1600..=1800).contains(&runs),
            "htop should batch to ~1760 style runs/frame, got {runs}"
        );
    }

    #[test]
    fn build_log_batches_to_a_few_runs_per_line() {
        // Level tag + bold counter + message per line, over 45 rows.
        let runs = run_count_after(Profile::BuildLog, 60);
        assert!(
            (90..=250).contains(&runs),
            "build-log should batch to ~130-180 style runs/frame, got {runs}"
        );
    }

    #[test]
    fn firehose_batches_to_about_one_run_per_line() {
        let runs = run_count_after(Profile::Firehose, 20);
        assert!(
            (30..=50).contains(&runs),
            "firehose is ~1 default-style run per row, got {runs}"
        );
    }

    #[test]
    fn typing_batches_to_at_most_one_run_per_row() {
        let runs = run_count_after(Profile::Typing, 40);
        assert!(
            runs <= usize::from(ROWS),
            "typing is default-style text only, got {runs}"
        );
        assert!(runs > 0, "40 typed chars must paint something");
    }

    // -----------------------------------------------------------------
    // Colour resolution
    // -----------------------------------------------------------------

    #[test]
    fn default_colors_fall_back_and_inverse_swaps_them_after_resolution() {
        let plain = CellStyle::default();
        let (fg, bg) = plain.resolve();
        assert_eq!(fg, DEFAULT_FG);
        assert_eq!(bg, None, "the default background is the whole-grid fill");

        // `ESC[7m` on otherwise-unstyled text must actually invert: the two
        // `Color::Default` sentinels resolve to their own fallbacks first, then
        // swap (resolving after the swap would leave this unchanged).
        let inverted = CellStyle {
            inverse: true,
            ..CellStyle::default()
        };
        let (fg, bg) = inverted.resolve();
        assert_eq!(fg, DEFAULT_BG);
        assert_eq!(bg, Some(DEFAULT_FG), "inverted runs paint a background");
    }

    #[test]
    fn dim_dulls_the_foreground_without_touching_the_background() {
        let dim = CellStyle {
            dim: true,
            fg: vt100::Color::Idx(2),
            ..CellStyle::default()
        };
        let (fg, bg) = dim.resolve();
        assert_eq!(fg, ANSI_16[2].with_alpha(DIM_ALPHA));
        assert_eq!(bg, None);
    }

    #[test]
    fn indexed_colors_cover_ansi_cube_and_greyscale() {
        assert_eq!(indexed_color(1), ANSI_16[1]);
        assert_eq!(indexed_color(15), ANSI_16[15]);
        // 16 is the cube origin (black), 231 its far corner (white).
        assert_eq!(indexed_color(16), Color::from_rgb8(0, 0, 0));
        assert_eq!(indexed_color(231), Color::from_rgb8(0xFF, 0xFF, 0xFF));
        // 232 is the darkest grey step, 255 the lightest.
        assert_eq!(indexed_color(232), Color::from_rgb8(8, 8, 8));
        assert_eq!(indexed_color(255), Color::from_rgb8(238, 238, 238));
    }

    #[test]
    fn sgr_colors_in_the_fixture_subset_resolve_to_the_ansi_palette() {
        // `ESC[31m` (red fg) / `ESC[44m` (blue bg) — the fixture subset.
        let batch = batch_of(b"\x1b[31;44mx");
        let runs = batch.active();
        assert_eq!(runs.len(), 1);
        let (fg, bg) = runs[0].style.resolve();
        assert_eq!(fg, ANSI_16[1]);
        assert_eq!(bg, Some(ANSI_16[4]));
    }

    // -----------------------------------------------------------------
    // Width-derived cell geometry
    // -----------------------------------------------------------------

    /// A synthetic monospace face: advance `ratio` x size, line box
    /// [`LINE_HEIGHT`] x size — linear, like a real unhinted monospace face.
    fn linear_face(ratio: f64) -> impl FnMut(f32) -> (f64, f64) {
        move |size| {
            (
                f64::from(size) * ratio,
                f64::from(size) * f64::from(LINE_HEIGHT),
            )
        }
    }

    #[test]
    fn geometry_solves_for_exactly_cols_columns_across_the_available_width() {
        // A phone's logical safe-area box.
        let available = Size::new(392.7, 872.7);
        let geometry = solve_cell_geometry(available, linear_face(0.6));

        // 80 columns land exactly on the available width — the whole point.
        assert!(
            (geometry.grid_width() - available.width).abs() < 0.01,
            "{geometry:?}"
        );
        // Width binds here, so the 45 rows letterbox rather than clip.
        assert!(geometry.grid_height() < available.height, "{geometry:?}");
        // 392.7 / (80 * 0.6) = 8.18125
        assert!(
            (geometry.font_size - 8.181_25).abs() < 0.001,
            "{geometry:?}"
        );
        assert!((geometry.width - 4.909).abs() < 0.001, "{geometry:?}");
    }

    #[test]
    fn geometry_is_derived_from_the_measured_advance_not_an_assumed_ratio() {
        // A 0.5em face and a 0.7em face must both fill exactly COLS columns —
        // i.e. the solver reads the ratio off the probe rather than assuming
        // 0.6em.
        let available = Size::new(400.0, 4000.0);
        for ratio in [0.5, 0.6, 0.7] {
            let geometry = solve_cell_geometry(available, linear_face(ratio));
            assert!(
                (geometry.grid_width() - available.width).abs() < 0.01,
                "ratio {ratio}: {geometry:?}"
            );
            assert!(
                (f64::from(geometry.font_size) - available.width / (f64::from(COLS) * ratio)).abs()
                    < 0.01,
                "ratio {ratio}: {geometry:?}"
            );
        }
    }

    #[test]
    fn a_short_wide_box_binds_on_height_instead_of_clipping_rows() {
        // A desktop preview window / landscape phone: 45 rows would overflow the
        // width-derived size, so height binds and the grid letterboxes at the
        // right instead of losing its bottom rows.
        let available = Size::new(1600.0, 600.0);
        let geometry = solve_cell_geometry(available, linear_face(0.6));
        assert!(
            (geometry.grid_height() - available.height).abs() < 0.01,
            "{geometry:?}"
        );
        assert!(geometry.grid_width() < available.width, "{geometry:?}");
    }

    #[test]
    fn a_face_whose_advance_quantizes_upward_still_never_overflows() {
        // A hypothetical face that rounds every advance up to a whole pixel: the
        // ratio-derived starting size overflows, so the shrink pass must bring it
        // back inside the box (this is why the solver re-measures).
        let available = Size::new(392.7, 872.7);
        let geometry = solve_cell_geometry(available, |size| {
            (
                (f64::from(size) * 0.6).ceil(),
                f64::from(size) * f64::from(LINE_HEIGHT),
            )
        });
        assert!(
            geometry.grid_width() <= available.width,
            "must fit: {geometry:?}"
        );
        assert!(
            geometry.grid_height() <= available.height,
            "must fit: {geometry:?}"
        );
    }

    #[test]
    fn an_unbounded_box_falls_back_instead_of_dividing_by_infinity() {
        let geometry =
            solve_cell_geometry(Size::new(f64::INFINITY, f64::INFINITY), linear_face(0.6));
        assert_eq!(geometry.font_size, FALLBACK_FONT_SIZE);
        // One bounded axis is still enough to derive from.
        let geometry = solve_cell_geometry(Size::new(392.7, f64::INFINITY), linear_face(0.6));
        assert!((geometry.grid_width() - 392.7).abs() < 0.01, "{geometry:?}");
    }

    #[test]
    fn a_box_too_narrow_for_cols_stops_at_the_font_size_floor() {
        // 80 columns cannot fit 100 logical px at any sane size. The solver floors
        // rather than shipping a 2px font.
        let geometry = solve_cell_geometry(Size::new(100.0, 872.7), linear_face(0.6));
        assert_eq!(geometry.font_size, MIN_FONT_SIZE);
        assert!(
            geometry.grid_width() > 100.0,
            "the floor is knowingly wider than the box: {geometry:?}"
        );
    }

    // -----------------------------------------------------------------
    // Transport: which feed loop is live
    // -----------------------------------------------------------------

    /// Install the reactive runtime + an ambient owner (needed for `RwSignal`,
    /// `on_cleanup`, and `spawn_local`; the runtime claims the calling thread as
    /// the UI thread, and a spawned local task only ever runs when pumped).
    fn setup() -> Owner {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let ambient = Owner::new();
        ambient.set();
        ambient
    }

    #[test]
    fn the_page_mounts_stopped_and_a_restart_retires_the_previous_loop() {
        let _owner = setup();
        let mut state = TerminalPage.init();
        // Nothing replays until a control is tapped: a section switch must not
        // start a 120 Hz firehose by itself.
        assert!(!state.running);
        assert_eq!(state.epoch.get(), 0);
        assert_eq!(state.profile, Profile::BuildLog);
        assert_eq!(state.generation.get_untracked(), 0);

        start_replay(&mut state, Profile::Htop);
        assert!(state.running);
        assert_eq!(state.profile, Profile::Htop);
        assert_eq!(state.epoch.get(), 1, "the new loop's epoch");
        let first = Rc::clone(&state.feed);

        // A profile switch is a full restart: a fresh emulator, and a bumped
        // epoch so the previous loop returns at its next wake instead of racing
        // the new one on the same signal.
        start_replay(&mut state, Profile::Typing);
        assert_eq!(state.profile, Profile::Typing);
        assert_eq!(state.epoch.get(), 2);
        assert!(!Rc::ptr_eq(&first, &state.feed), "a fresh feed");

        stop_replay(&mut state);
        assert!(!state.running);
        assert_eq!(state.epoch.get(), 3, "the live loop is retired");

        // Every transport tap also wakes the rebuild exactly once.
        assert_eq!(state.generation.get_untracked(), 3);
    }

    #[test]
    fn one_pump_of_the_feed_loop_feeds_the_first_due_chunk_and_publishes_once() {
        // The wiring the on-device replay rides: `spawn_local` puts the loop on
        // the UI-thread queue, and the shell's per-frame `pump_local` runs it.
        // The first iteration is deterministic — it feeds everything due at
        // t≈0 (chunk 0) and publishes ONE generation before awaiting its sleep,
        // so this needs no timing assumption at all.
        let _owner = setup();
        let runtime = ReactiveRuntime::get().expect("initialised by setup()");
        let mut state = TerminalPage.init();

        start_replay(&mut state, Profile::BuildLog);
        let after_start = state.generation.get_untracked();
        assert_eq!(state.feed.borrow().fed_chunks(), 0, "nothing has run yet");

        runtime.pump_local();
        assert_eq!(
            state.feed.borrow().fed_chunks(),
            1,
            "the chunk due at t=0 reached the emulator"
        );
        assert_eq!(
            state.generation.get_untracked(),
            after_start + 1,
            "one signal write per tick, never one per chunk"
        );

        // A stop retires that loop: a later pump must not feed anything more.
        stop_replay(&mut state);
        runtime.pump_local();
        assert_eq!(state.feed.borrow().fed_chunks(), 1, "the loop returned");
    }

    // -----------------------------------------------------------------
    // Widget layout/paint smoke test (no GPU, no window)
    // -----------------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        glyph_runs: usize,
        clips: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, origin: Point, size: Size, color: Color) {
            self.rects.push((origin, size, color));
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: frust_scene::GlyphRun) {
            self.glyph_runs += 1;
        }
        fn push_clip(&mut self, _origin: Point, _size: Size) {
            self.clips += 1;
        }
        fn pop_clip(&mut self) {}
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _w: f64, _c: Color) {}
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
    }

    /// Build the grid widget over a feed pre-loaded with `chunks` chunks.
    fn grid_at(profile: Profile, chunks: usize) -> (TerminalGridWidget, usize) {
        let mut feed = TerminalFeed::new(profile);
        let interval = profile.interval_ms().unwrap_or(0.0);
        feed.feed_due(Duration::from_secs_f64(
            (chunks as f64 * interval) / 1000.0 + 0.001,
        ));
        let expected = {
            let mut batch = RunBatch::default();
            batch_screen(feed.screen(), &mut batch);
            batch.count()
        };
        let view = TerminalGridView {
            generation: 1,
            feed: Rc::new(RefCell::new(feed)),
            stats: Rc::new(Cell::new(GridStats::default())),
        };
        let mut counter = 0u64;
        let widget = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        (widget, expected)
    }

    #[test]
    fn grid_shapes_one_glyph_run_per_batched_run_and_clips_itself() {
        let (mut widget, expected) = grid_at(Profile::BuildLog, 60);
        assert!(expected > 0);

        let mut tcx = RawTextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let available = Size::new(392.7, 872.7); // a phone's logical safe-area box
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(available));
        // The grid is exactly COLS x ROWS cells, and it fits the content box: no
        // clipped column, no row beyond the 45th.
        let (_, geometry) = widget.geometry.expect("geometry solved during layout");
        assert!((size.width - geometry.grid_width()).abs() < 0.01);
        assert!((size.height - geometry.grid_height()).abs() < 0.01);
        assert!(size.width <= available.width + 0.01, "{size:?}");
        assert!(size.height <= available.height + 0.01, "{size:?}");
        assert_eq!(widget.batch.count(), expected);

        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        widget.paint(&mut pctx, &mut rec);

        assert_eq!(rec.clips, 1, "the grid clips its own overflow");
        assert_eq!(rec.rects[0].2, DEFAULT_BG, "backdrop fill first");
        // Each batched run shapes to >= 1 GlyphRun (one per style span parley
        // emits; monospace ASCII is one).
        assert!(
            rec.glyph_runs >= expected,
            "{} glyph runs for {expected} batched runs",
            rec.glyph_runs
        );
        assert!(
            !pctx.needs_frame(),
            "the grid never requests frames — the feed loop is the only wake source"
        );
        // The page's readout reads exactly what paint published.
        let stats = widget.stats.get();
        assert_eq!(stats.runs, expected);
        assert_eq!(stats.painted_frames, 1);
    }

    #[test]
    fn idle_profile_paints_only_the_backdrop_and_requests_nothing() {
        let (mut widget, expected) = grid_at(Profile::Idle, 0);
        assert_eq!(expected, 0);

        let mut tcx = RawTextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(2000.0, 2000.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        widget.paint(&mut pctx, &mut rec);

        assert_eq!(rec.glyph_runs, 0, "nothing to paint at rest");
        assert_eq!(rec.rects.len(), 1, "the backdrop fill only");
        assert!(!pctx.needs_frame(), "rest cost is zero frames");
    }

    #[test]
    fn a_new_generation_forces_layout_not_just_paint() {
        // The reshape happens in `layout`, so a bare PAINT would freeze the grid
        // under Android's intra-frame layout-skip gate.
        let feed = Rc::new(RefCell::new(TerminalFeed::new(Profile::BuildLog)));
        let stats = Rc::new(Cell::new(GridStats::default()));
        let make = |generation: u64| TerminalGridView {
            generation,
            feed: Rc::clone(&feed),
            stats: Rc::clone(&stats),
        };
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let v0 = make(0);
        let mut widget = View::<()>::build(&v0, &mut ctx);

        let same = View::<()>::rebuild(&make(0), &v0, &mut widget, &mut ctx);
        assert_eq!(same, ChangeFlags::NONE, "an unchanged generation is free");

        let bumped = View::<()>::rebuild(&make(1), &v0, &mut widget, &mut ctx);
        assert!(bumped.needs_layout(), "a new generation must relayout");
        assert_eq!(widget.generation, 1);
    }

    #[test]
    fn a_restart_swaps_the_feed_even_at_the_same_generation() {
        // A profile switch replaces the whole feed; the grid must pick the new one
        // up rather than keep painting the retired emulator's screen.
        let stats = Rc::new(Cell::new(GridStats::default()));
        let first = Rc::new(RefCell::new(TerminalFeed::new(Profile::BuildLog)));
        let second = Rc::new(RefCell::new(TerminalFeed::new(Profile::Htop)));
        let view_of = |feed: &Rc<RefCell<TerminalFeed>>| TerminalGridView {
            generation: 7,
            feed: Rc::clone(feed),
            stats: Rc::clone(&stats),
        };
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let v0 = view_of(&first);
        let mut widget = View::<()>::build(&v0, &mut ctx);

        let flags = View::<()>::rebuild(&view_of(&second), &v0, &mut widget, &mut ctx);
        assert!(flags.needs_layout(), "a swapped feed must relayout");
        assert!(Rc::ptr_eq(&widget.feed, &second));
    }

    #[test]
    fn reshaping_is_skipped_while_the_generation_is_unchanged() {
        let (mut widget, _) = grid_at(Profile::BuildLog, 60);
        let mut tcx = RawTextContext::new();
        let bc = BoxConstraints::loose(Size::new(2000.0, 2000.0));
        {
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(&mut lctx, &bc);
        }
        let shapes_after_first = tcx.shape_cache_stats().shapes;
        {
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(&mut lctx, &bc);
        }
        assert_eq!(
            tcx.shape_cache_stats().shapes,
            shapes_after_first,
            "a second layout at the same generation must not re-shape"
        );
    }

    #[test]
    fn a_real_shaped_face_fills_the_width_and_a_box_change_re_solves() {
        let (mut widget, _) = grid_at(Profile::BuildLog, 60);
        let mut tcx = RawTextContext::new();

        let mut layout_at = |available: Size, widget: &mut TerminalGridWidget| {
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(&mut lctx, &BoxConstraints::loose(available))
        };

        // A real monospace face (whatever the host resolves for the `Monospace`
        // generic slot) must fill the width to within one cell.
        let phone = Size::new(392.7, 872.7);
        let size = layout_at(phone, &mut widget);
        let (_, geometry) = widget.geometry.expect("solved");
        assert!(size.width <= phone.width + 0.01, "{size:?}");
        assert!(
            phone.width - size.width < geometry.width,
            "less than one cell of unused width: {size:?} at {geometry:?}"
        );

        // A wider box (rotation / a resized desktop window) re-solves rather than
        // keeping the old cell size, and re-shapes at the new size.
        let wide = Size::new(700.0, 872.7);
        let resized = layout_at(wide, &mut widget);
        let (_, resized_geometry) = widget.geometry.expect("re-solved");
        assert!(resized.width > size.width, "{resized:?} vs {size:?}");
        assert!(resized_geometry.font_size > geometry.font_size);
        assert!(resized.width <= wide.width + 0.01, "{resized:?}");
        assert!(resized.height <= wide.height + 0.01, "{resized:?}");
    }
}
