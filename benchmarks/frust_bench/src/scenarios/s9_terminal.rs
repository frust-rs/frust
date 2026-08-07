//! S9 — Terminal grid stream (fixture replay through a real VT emulator).
//!
//! The frust side of the muxr terminal-port decision: replay a checked-in
//! byte stream through a **real** terminal emulator (the `vt100` crate) into a
//! batched, painted 80x45 character grid, and measure what the frame costs.
//! The Flutter side (`benchmarks/flutter_bench/lib/scenarios/s9_terminal.dart`)
//! replays the **same bytes at the same cadence** through **kterm 1.5.3** — the
//! renderer muxr ships today — so this is a candidate-versus-known-good
//! measurement, not a microbenchmark.
//!
//! # Fairness gate: the fixtures are data, never generated at runtime
//!
//! Both apps embed `benchmarks/harness/fixtures/terminal/<profile>.chunks`
//! verbatim (`include_bytes!` here; an asset on the Dart side) and replay it
//! under one contract:
//!
//! ```text
//! <profile>.chunks   repeat [u32 little-endian length][length bytes]
//!                    no header, no trailer, no padding
//! replay             emit chunk i at t = i * (1000 / hz) ms from start
//! ```
//!
//! The same convention S6 uses for its multilingual `CORPUS`, moved to a file
//! because the volume is far too large for a source literal. See that
//! directory's `README.md` for the escape-sequence subset and the per-profile
//! rationale; [`Profile`] mirrors its table.
//!
//! # Why a real emulator, not a hand-rolled parser
//!
//! kterm carries a full VT emulator. Comparing it against a five-escape
//! hand-rolled parser would measure a renderer against a strawman. `vt100`
//! (MIT, deps `vte`/`itoa`/`unicode-width` only — no tokio, no PTY, no
//! `std::process`) is the emulator here, which also settles a standing open
//! question: **no VT crate had been verified to build for
//! `aarch64-linux-android` / `aarch64-apple-ios` before this scenario.** It
//! does (see `Cargo.toml`'s `vt100` pin for the exact versions gated).
//!
//! # The 30 Hz coalescing mechanism (research finding 1)
//!
//! `FrameGate`'s pacing gate ([`frust_shell_common::frame_gate`]'s
//! `FrameInputs::is_paced_only_frame`) requires `!signals_dirty`, so **a signal
//! write can never be paced by the framework** — every write forces an unpaced
//! frame at panel rate, and `MotionScheme::cosmetic_loop_rate` only ever paces
//! animation-only frames. A terminal that wrote a signal per chunk would
//! therefore run `firehose` at 120 forced frames/second. That is not a design
//! anybody would ship, so the coalescing happens **app-side**, exactly as it
//! would in muxr:
//!
//! - A UI-thread `spawn_local` loop ([`run_feed`]) wakes every
//!   [`REPAINT_INTERVAL`] (1/[`REPAINT_HZ`] = 33.3 ms), feeds **every chunk
//!   whose scheduled time has passed** into the emulator, and then writes the
//!   generation signal **once** — one write per tick, never one per chunk. No
//!   input byte is ever dropped or delayed past its schedule; only the
//!   *repaint* is coalesced.
//! - Between ticks nothing is dirty and nothing requests a frame, so the mobile
//!   frame gate skips. The `idle` profile writes the signal zero times and
//!   therefore produces zero frames after mount — that is the rest-cost
//!   measurement, not a bug.
//! - The wake source is a real timer, not a per-frame `PaintCtx::request_frame`:
//!   the loop `await`s a [`frust::spawn_blocking`] sleep, and both mobile shells
//!   drain the UI-thread local task queue (`ReactiveRuntime::pump_local`) at the
//!   **top** of every tick, before the frame-gate decision. So a tick that only
//!   sleeps costs one pump and one gate `Skip`. (`spawn_blocking` + a std sleep
//!   rather than `tokio::time::sleep` keeps this scenario on facade APIs the
//!   other scenarios already use, adding no dependency beyond `vt100`.)
//! - Emulator parsing runs **on the UI thread**, deliberately: kterm parses on
//!   Dart's UI isolate, so moving frust's parse off-thread would flatter frust
//!   on an axis the comparison is not about.
//!
//! **Changing the cap** is one constant: [`REPAINT_HZ`]. Nothing else derives
//! from it. Raising it above the panel rate makes every tick a forced frame
//! (the unthrottled strawman); lowering it trades latency for frames.
//!
//! ## Desktop reads higher than mobile — do not compare the two
//!
//! The mobile gate keys on `signals_dirty`, which only a *signal write* trips;
//! a bare `spawn_local` wake does not. So on Android/iOS a tick that merely
//! polled the sleeping loop skips, and **frames/second converges on the
//! generation rate** — 10/s for `htop`, ~30/s (the cap) for `build-log` and
//! `firehose`. The desktop shell has no `FrameGate`: its `FrameWaker` fires on
//! **every** local-task wake, so a desktop run repaints at [`REPAINT_HZ`]
//! regardless of profile (measured: `htop` produced ~30 fps on desktop against
//! ~10 generations/s). Desktop is the smoke test; only the device runs are the
//! measurement.
//!
//! # Same-style run batching (research finding 2)
//!
//! Each [`frust_scene::GlyphRun`] becomes exactly one vello `draw_glyphs` call —
//! the scene layer does no batching — and `TextContext`'s shape cache is a
//! bounded **128-entry** LRU keyed on `(text, style)`. kterm batches consecutive
//! same-style characters into one `Paragraph`; [`batch_screen`] batches
//! identically: consecutive cells sharing a whole [`CellStyle`] (fg, bg, bold,
//! italic, underline, inverse, dim) coalesce into **one** shaped run and **one**
//! background rectangle. There is never a draw per cell.
//!
//! Run counts per painted frame at 80x45 — the primary interpretation aid,
//! since research finding 3 says vello's cost tracks scene-derived counters
//! (i.e. run count) and **not** surface resolution. Measured on the desktop
//! preview (2026-07-30) and pinned by this module's unit tests:
//!
//! | profile | style runs/frame | why |
//! |---|---|---|
//! | `idle` | 0 | nothing painted but the backdrop |
//! | `typing` | <= 45 (grows to 1/row) | one default-style run per non-blank row |
//! | `build-log` | **132** (steady) | 3 visible runs/line: level tag, bold counter, message |
//! | `htop` | **1722-1745** | a fg+bg pair change every 2 cells, 40 runs/row x 44 visible rows |
//! | `firehose` | **44** | one default-style run per line |
//!
//! Each run costs one `TextContext::layout` (a shape-cache miss on
//! `htop`/`firehose`, whose text is fresh every frame), one background rect when
//! its bg is non-default, and one vello `draw_glyphs`. `htop` therefore asks for
//! ~1735 shapes + ~1735 rects + ~1735 glyph draws per frame, at 10 Hz.
//!
//! The live counts are logged once per second (see [`LOG_PREFIX`]) whenever
//! `perf::enabled()`, so a captured run carries its own run-count series.
//!
//! # Width-derived cell geometry (the S9 fairness fix, 2026-07-30)
//!
//! The first device pass (OnePlus 9, 1080x2400) caught the two apps painting
//! **geometrically different grids**, which invalidated the comparison
//! outright:
//!
//! | | frust (before) | Flutter/kterm (before) |
//! |---|---|---|
//! | font size | 10.0 logical px (a hardcoded const) | 13 px (kterm's default `TerminalStyle()`) |
//! | rows painted | 45 | ~49 — `TerminalView` filled the viewport height and pulled in scrollback |
//! | columns visible | ~62 of 80 (clipped) | ~48 of 80 (clipped) |
//! | screen coverage | top ~55%, black below | full height |
//!
//! Both errors flattered **frust** (fewer rasterized pixels for the same
//! logical work), i.e. they pushed toward "a pure-frust terminal is viable" on
//! a false premise. The fix is applied identically on both sides:
//!
//! 1. Measure the available content box (the safe-area box: `bc.max()` here,
//!    a `LayoutBuilder`'s constraints on the Dart side).
//! 2. Solve for the font size at which **exactly [`COLS`] columns fits that
//!    width**, using each engine's **own measured** monospace advance — never
//!    an assumed 0.6em ratio ([`solve_cell_geometry`]).
//! 3. Paint exactly [`ROWS`] rows at the resulting cell height, anchored top,
//!    with the painted region constrained to exactly `COLS * cell_w` by
//!    `ROWS * cell_h` so nothing outside the grid can paint.
//! 4. Leftover vertical space stays empty — letterboxed identically in both.
//!
//! Two residual asymmetries this does **not** remove, recorded rather than
//! hidden:
//!
//! - **Cull order differs by one row.** This widget batches and shapes all
//!   [`ROWS`] rows and leans on its clip (shape-then-clip); kterm culls to the
//!   line range its viewport covers (clip-then-shape) from an *inclusive*
//!   truncating-division range, so it can shape one extra row its own
//!   `ClipRect` then discards. With the whole grid visible both walk the same
//!   45 rows, ±1 clipped line on the Flutter side.
//! - **Neither engine culls columns.** frust shapes every batched run; kterm
//!   paints every cell of a line. That was the expensive half of the old defect
//!   — ~32 hidden columns shaped and submitted on both sides — and with 80
//!   columns now visible there is nothing hidden left to pay for.
//!
//! The derived size is device-dependent, which is the point; what must hold is
//! that **both apps derive the same one on the same device**. That is why both
//! now log their computed geometry ([`LOG_PREFIX`]) — the equality is meant to
//! be read off a capture, never assumed. On the OP9's ~392.7 x ~872.7 logical
//! safe-area box the derivation lands at ~8.18 px font, ~4.91 px cell width,
//! ~9.82 px cell height, 80x45 fully visible (~442 of ~873 logical px tall,
//! the rest letterboxed).
//!
//! Solving from a *measured* advance is what keeps the two apps in agreement
//! without sharing a font file: frust shapes IBM Plex Mono (advance 0.6 em),
//! kterm shapes the platform `monospace` face (Roboto Mono on Android,
//! 0.60009765625 em), so the same derivation puts both cell widths at exactly
//! `width / COLS` and both font sizes within ~0.02% of each other.
//!
//! The solver also fits the **height** (`ROWS * cell_h <= available height`),
//! so a short/wide viewport — the desktop preview window, or a landscape phone
//! — shrinks the grid to fit instead of clipping its bottom rows. On every
//! device in the S9 matrix (portrait phones) width is the binding axis; the
//! diagnostic line's `cell_w`/`cell_h` make it checkable either way.
//!
//! # Profile selection (must be pinned into `PROTOCOL.md`)
//!
//! The harness selects a *scenario*; S9 needs a second axis, and the two
//! platforms have different delivery capabilities — the same asymmetry
//! `PROTOCOL.md` §1 already documents for whole-scenario selection. Resolved
//! once at [`Component::init`], first match wins:
//!
//! 1. **Deep-link path segment** — `frustbench://s9/<profile>`. The primary
//!    on-device path: `adb shell am start -a android.intent.action.VIEW -d
//!    "frustbench://s9/htop" <pkg>` (what `harness/run.sh` already does for
//!    scenario selection, with a path appended), or `xcrun simctl openurl
//!    booted "frustbench://s9/htop"`. `scenarios::index_from_url` already
//!    truncates the host at the first `/`, so `s9` still resolves — no harness
//!    or `Scenario`-trait change, and **no rebuild per profile**.
//! 2. **Runtime env var [`PROFILE_ENV`]** (`FRUST_BENCH_S9_PROFILE`) — read the
//!    same way `scenarios::SCENARIO_ENV` is. The desktop path, and the physical
//!    iPhone path (`devicectl device process launch -e
//!    '{"FRUST_BENCH_S9_PROFILE":"htop",...}'`, which has no deep-link
//!    trigger).
//! 3. **Compile-time define** of the same name (`option_env!`), for
//!    `frust build --define FRUST_BENCH_S9_PROFILE=htop`. Last resort — an
//!    Android release/profile build has been observed to drop `--define`
//!    (`docs/DEVELOPMENT.md`'s Known Issues), which is exactly why mechanism 1
//!    is the on-device primary.
//! 4. **[`DEFAULT_PROFILE`]** — `build-log`, the realistic workload and primary
//!    verdict input.
//!
//! Accepted profile strings are exactly the fixture file stems: `idle`,
//! `typing`, `build-log`, `htop`, `firehose`.
//!
//! # Markers
//!
//! [`S9::on_start`]/[`on_end`](S9::on_end) stamp the plain `s9` window the
//! harness slices by, then a `s9-<profile>` sub-marker inside it — the same
//! finer-sub-marker latitude S3 uses for its per-op windows (see
//! `super`'s marker contract).
//!
//! # Deliberate non-features
//!
//! No cursor, no selection, no scrollback view, no reflow-on-resize, and no
//! `Widget::semantics` impl (a 3600-character label rebuilt every frame would
//! pollute the very measurement this scenario exists to take). The grid stays
//! fixed at [`COLS`]x[`ROWS`] cells — the fixture contract — anchored top-left
//! in the safe area, with the cell size derived from the safe-area width (see
//! the width-derived cell geometry section above) so both apps rasterize the
//! same cell count over the same painted area.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use frust::authoring::text::{
    FontFamily, FontStyle, FontWeight, GenericSlot, LineHeight, TextContext, TextLayout, TextStyle,
};
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Point, Size, View,
    Widget,
};
use frust::{
    Align, Alignment, AnyView, Color, Component, FrameTime, Get, GetUntracked, RwSignal, Set, any,
    component, deep_links, on_cleanup, spawn_blocking, spawn_local,
};

use super::{BenchState, Scenario};

// ---------------------------------------------------------------------------
// Shared constants — the numbers the Flutter side must match
// ---------------------------------------------------------------------------

/// Grid width in cells. Fixed by the fixture contract, never derived from the
/// screen, so both apps rasterize the same cell count.
pub const COLS: u16 = 80;
/// Grid height in cells. Fixed by the fixture contract (see [`COLS`]).
pub const ROWS: u16 = 45;

/// Cell height as a multiple of the derived font size (CSS unitless
/// `line-height`) — 1.2, the same value kterm's `TerminalStyle.height` defaults
/// to, so both apps' row pitch derives from their font size the same way.
///
/// The *measured* line box (what actually spaces the rows) comes back from the
/// shaper, not from this constant: see [`solve_cell_geometry`].
pub const LINE_HEIGHT: f32 = 1.2;

/// Font size used **only** when neither axis of the content box is bounded, i.e.
/// when the width-derived geometry has nothing to solve against (a degenerate
/// parent, or a unit test with unbounded constraints). Every real frame derives
/// its size from the content box instead — see the module docs' width-derived
/// cell geometry section.
pub const FALLBACK_FONT_SIZE: f32 = 10.0;

/// Floor on the derived font size. Reaching it means [`COLS`] columns no longer
/// fit the content box at a size either text engine renders sanely (a viewport
/// under ~192 logical px wide at a 0.6em advance), at which point the grid
/// clips again and the run is **not** comparable — the honest fix there is a
/// narrower grid in both apps plus regenerated fixtures, not a smaller font.
/// No device in the S9 matrix comes near this.
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
/// width can measure back a hundred-thousandth of a pixel over it; treating
/// that as an overflow would send the solver chasing float jitter (and, with
/// [`FIT_BACKOFF`], shrink the grid ~10% for it). One hundredth of a logical
/// pixel is ~0.03 device px on a 2.75-density screen — sub-pixel rounding on the
/// last column's right edge, not a clipped column.
const FIT_TOLERANCE: f64 = 0.01;

/// The app-side repaint coalescing cap, in Hz — **the single knob** for the
/// mechanism the module docs describe. 30 Hz is the rate a terminal UI would
/// ship at; the framework cannot pace a signal-driven repaint for us (research
/// finding 1).
pub const REPAINT_HZ: f64 = 30.0;

/// [`REPAINT_HZ`] as a period — derived, so [`REPAINT_HZ`] really is the only
/// knob. The feed loop wakes on this grid, feeds every chunk already due, and
/// publishes at most one generation per wake.
pub const REPAINT_INTERVAL: Duration = Duration::from_nanos((1_000_000_000.0 / REPAINT_HZ) as u64);

/// Env var / compile-time define naming the profile to replay (mechanisms 2
/// and 3 in the module docs). Read exactly the way
/// [`super::SCENARIO_ENV`] is, plus an `option_env!` fallback.
pub const PROFILE_ENV: &str = "FRUST_BENCH_S9_PROFILE";

/// The profile used when nothing selects one: the realistic workload and the
/// primary verdict input.
pub const DEFAULT_PROFILE: Profile = Profile::BuildLog;

/// Prefix of the once-per-second diagnostic line (`log::info!`, gated on
/// `perf::enabled()`). Deliberately **not** `frust-perf` so it can never be
/// confused with a shell-emitted raw frame line by `stats.py`.
pub const LOG_PREFIX: &str = "bench-s9";

/// How often the diagnostic line above is emitted.
const LOG_INTERVAL: Duration = Duration::from_secs(1);

/// Probe string whose shaped width, divided by its length, yields the monospace
/// cell advance. Ten ASCII digits: all one cell wide in every monospace face,
/// and long enough that a per-glyph quantization rounding is visible in the
/// average if it ever stops being uniform.
const CELL_PROBE: &str = "0123456789";

// ---------------------------------------------------------------------------
// Palette — literal, so the Flutter side can match it exactly
// ---------------------------------------------------------------------------

/// Default foreground (SGR 39 / never-set). A literal rather than a `GlyphInk`
/// token: the Dart side has no access to frust's theme, and a byte-identical
/// palette on both sides is worth more here than token fidelity.
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

/// Alpha applied to the foreground for SGR 2 (dim). Not exercised by the
/// fixture subset — carried so an out-of-subset byte renders rather than
/// silently diverging from kterm.
const DIM_ALPHA: f32 = 0.66;

/// Underline stroke width in logical px, and its offset below the cell's top
/// edge as a fraction of the cell height.
const UNDERLINE_WIDTH: f64 = 1.0;
const UNDERLINE_Y_FRACTION: f64 = 0.92;

// ---------------------------------------------------------------------------
// Profiles — one per checked-in fixture
// ---------------------------------------------------------------------------

/// The five replay profiles, mirroring
/// `benchmarks/harness/fixtures/terminal/README.md`'s table. Each variant's
/// bytes are embedded from that directory; `hz` and `chunk_count` are the
/// matching `<profile>.json` manifest values, asserted against the embedded
/// bytes by this module's tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// No output at all — rest cost. The frame gate should idle at zero frames.
    Idle,
    /// One printable char per chunk at 5 Hz — the minimum update (one cell).
    Typing,
    /// One scrolling log line per chunk at 30 Hz — the realistic workload and
    /// the primary verdict input.
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

    /// The fixture file stem, and the accepted selector string (deep-link path
    /// segment / env value).
    pub fn name(self) -> &'static str {
        match self {
            Profile::Idle => "idle",
            Profile::Typing => "typing",
            Profile::BuildLog => "build-log",
            Profile::Htop => "htop",
            Profile::Firehose => "firehose",
        }
    }

    /// Parse a selector string ([`name`](Self::name)), case-insensitively.
    pub fn from_name(s: &str) -> Option<Profile> {
        Profile::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(s))
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

    /// The manifest's `chunk_count`. Walking [`chunks`](Self::chunks) must
    /// yield exactly this many chunks and consume every byte — the fixture
    /// integrity gate this module's tests enforce.
    pub fn chunk_count(self) -> usize {
        match self {
            Profile::Idle => 0,
            Profile::Typing => 150,
            Profile::BuildLog => 900,
            Profile::Htop => 300,
            Profile::Firehose => 3600,
        }
    }

    /// The embedded fixture bytes.
    ///
    /// Embedded rather than read from disk so Android's asset layout and iOS's
    /// bundle layout need no per-platform loader, and so a run can never
    /// silently replay a stale/missing file. Cost: ~7.5 MB of fixture bytes in
    /// the binary, dominated by `htop` (5.5 MB) and `firehose` (1.9 MB). That is
    /// acceptable for a debug/profile bench artifact; were it ever not, the
    /// alternative is a `[features]` split (one feature per profile, one
    /// artifact per profile) rather than shrinking the fixture set, which would
    /// break comparability with `RESULTS.md`.
    pub fn chunks(self) -> &'static [u8] {
        match self {
            Profile::Idle => include_bytes!("../../../harness/fixtures/terminal/idle.chunks"),
            Profile::Typing => include_bytes!("../../../harness/fixtures/terminal/typing.chunks"),
            Profile::BuildLog => {
                include_bytes!("../../../harness/fixtures/terminal/build-log.chunks")
            }
            Profile::Htop => include_bytes!("../../../harness/fixtures/terminal/htop.chunks"),
            Profile::Firehose => {
                include_bytes!("../../../harness/fixtures/terminal/firehose.chunks")
            }
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

/// Resolve the profile from a `frustbench://s9/<profile>` URL (mechanism 1).
///
/// Pure so it is testable without a reactive runtime. Requires the host to be
/// exactly `s9` — a link naming another scenario must never silently retarget
/// this one.
pub fn profile_from_url(url: &str) -> Option<Profile> {
    let rest = url.strip_prefix(&format!("{}://", super::DEEP_LINK_SCHEME))?;
    let mut parts = rest.split(['/', '?', '#']);
    if parts.next()? != "s9" {
        return None;
    }
    let segment = parts.next()?;
    Profile::from_name(segment.trim())
}

/// Process-wide cache of the resolved profile — see [`resolve_profile`].
static RESOLVED_PROFILE: OnceLock<Profile> = OnceLock::new();

/// The active profile, resolved once per process and cached.
///
/// Cached because the three callers ([`S9::on_start`], [`S9::on_end`],
/// [`S9Terminal::init`]) must agree: resolving independently would let a later
/// warm deep link naming a *different* scenario fall through to the env/default
/// and close an `s9-htop` marker window with an `s9-build-log` end marker. One
/// profile per process run is also the harness's actual usage (a launch per
/// profile), and a mid-run profile switch is explicitly not supported.
///
/// First call wins, first match wins within it: deep link, runtime env,
/// compile-time define, [`DEFAULT_PROFILE`] (see the module docs). The
/// deep-link slot is read **untracked** — this must never subscribe a caller's
/// rebuild to `latest`.
fn resolve_profile() -> Profile {
    *RESOLVED_PROFILE.get_or_init(resolve_profile_uncached)
}

/// The uncached resolution chain behind [`resolve_profile`].
fn resolve_profile_uncached() -> Profile {
    let links = deep_links();
    if let Some(p) = links
        .latest
        .get_untracked()
        .as_ref()
        .and_then(|l| profile_from_url(&l.url))
    {
        return p;
    }
    if let Some(p) = links.initial.as_deref().and_then(profile_from_url) {
        return p;
    }
    if let Ok(v) = std::env::var(PROFILE_ENV)
        && let Some(p) = Profile::from_name(v.trim())
    {
        return p;
    }
    if let Some(v) = option_env!("FRUST_BENCH_S9_PROFILE")
        && let Some(p) = Profile::from_name(v.trim())
    {
        return p;
    }
    DEFAULT_PROFILE
}

// ---------------------------------------------------------------------------
// Chunk stream reader (pure)
// ---------------------------------------------------------------------------

/// Read the next `[u32 le length][length bytes]` chunk at `cursor`, advancing
/// it past the payload. `None` on a clean end **or** a truncated tail (a
/// truncated fixture must stop the replay, never panic mid-capture).
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
    /// add allocation noise to the measurement).
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
    /// returning how many were fed. This is the "coalesce the repaint, never
    /// drop input bytes" half of the mechanism: a late wake catches up on all
    /// due chunks in one pass rather than skipping any.
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

    /// Chunks fed so far, for the diagnostic line.
    pub fn fed_chunks(&self) -> usize {
        self.fed_chunks
    }

    /// Bytes fed so far, for the diagnostic line.
    pub fn fed_bytes(&self) -> usize {
        self.fed_bytes
    }
}

// ---------------------------------------------------------------------------
// Style key and run batching (pure — the fairness-critical half)
// ---------------------------------------------------------------------------

/// Everything about a cell that affects how it paints. Two consecutive cells
/// batch into one run iff their `CellStyle`s are equal — the kterm-parity
/// contract (research finding 2).
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
    /// `Color::Default`s by slot and leave `ESC[7m` on an otherwise-unstyled
    /// cell painting light-on-dark, i.e. not inverted at all.
    fn resolve(self) -> (Color, Option<Color>) {
        let fg = resolve_color(self.fg, DEFAULT_FG);
        let bg = resolve_color(self.bg, DEFAULT_BG);
        let (mut fg, bg) = if self.inverse { (bg, fg) } else { (fg, bg) };
        if self.dim {
            fg = fg.with_alpha(DIM_ALPHA);
        }
        (fg, (bg != DEFAULT_BG).then_some(bg))
    }

    /// The shaping style for this run's text at the derived `font_size` (see
    /// the module docs' width-derived cell geometry).
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
/// 232-255 a 24-step greyscale ramp. The fixture subset only ever emits 0-7,
/// but a wrong-looking fallback for the rest would be a silent divergence from
/// kterm rather than a visible one.
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
/// retained empties whose `String` allocations get reused next frame. Rebuilding
/// the batch every generation must not re-allocate 1800 `String`s.
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

    /// How many runs the current frame batched to.
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
    /// blanks, so those are trimmed; a run that is then empty (a blank stretch
    /// on the default background — most of a terminal screen most of the time)
    /// is discarded, and its slot is reused by the next `open`. A run with a
    /// non-default background always commits: its rect paints even where its
    /// text does not.
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

/// Batch one emulator screen into same-style runs (research finding 2).
///
/// Walks each row left to right, coalescing consecutive cells whose whole
/// [`CellStyle`] matches into one run. A cell with no contents contributes a
/// single space so the following glyphs stay on their columns; the second half
/// of a wide character contributes nothing of its own (its glyph belongs to the
/// first half, which spans two columns instead of one). The fixture subset is
/// pure ASCII, so the wide path is correctness-for-free rather than something
/// this benchmark exercises.
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
/// chunk, publish **one** generation, repeat until the stream drains or the
/// component tears down.
///
/// Every `await` point releases the [`RefCell`] borrow first — a borrow held
/// across a suspension would panic the moment the grid widget laid out.
async fn run_feed(
    feed: Rc<RefCell<TerminalFeed>>,
    generation: RwSignal<u64>,
    cancelled: Arc<AtomicBool>,
) {
    let start = Instant::now();
    let mut published = 0u64;
    let mut tick = 0u32;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let fed = feed.borrow_mut().feed_due(start.elapsed());
        if fed > 0 {
            // ONE signal write per wake, never one per chunk: this is the whole
            // point of the mechanism (see the module docs' finding 1).
            published += 1;
            generation.set(published);
        }
        if feed.borrow().is_drained() {
            return;
        }
        // Sleep to the next absolute grid point rather than "now + interval",
        // so a slow frame can't drift the replay schedule.
        tick += 1;
        let deadline = REPAINT_INTERVAL * tick;
        let elapsed = start.elapsed();
        if let Some(nap) = deadline.checked_sub(elapsed) {
            let _ = spawn_blocking(move || std::thread::sleep(nap)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Scenario + Component
// ---------------------------------------------------------------------------

/// S9 — Terminal grid stream.
pub struct S9;

impl Scenario for S9 {
    fn id(&self) -> &'static str {
        "s9"
    }

    fn title(&self) -> &'static str {
        "Terminal grid stream"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(S9Terminal))
    }

    /// Open the plain `s9` window the harness slices by, then a
    /// `s9-<profile>` sub-marker inside it (S3's finer-sub-marker precedent).
    fn on_start(&self) {
        frust_shell_common::perf::mark_scenario_start(self.id());
        frust_shell_common::perf::mark_scenario_start(profile_marker(resolve_profile()));
    }

    fn on_end(&self) {
        frust_shell_common::perf::mark_scenario_end(profile_marker(resolve_profile()));
        frust_shell_common::perf::mark_scenario_end(self.id());
    }
}

/// The `s9-<profile>` sub-marker name. `&'static str` (the marker API takes
/// one), so this is a match rather than a `format!`.
fn profile_marker(profile: Profile) -> &'static str {
    match profile {
        Profile::Idle => "s9-idle",
        Profile::Typing => "s9-typing",
        Profile::BuildLog => "s9-build-log",
        Profile::Htop => "s9-htop",
        Profile::Firehose => "s9-firehose",
    }
}

/// S9's component: owns the emulator, the generation signal, and the feed loop.
pub struct S9Terminal;

/// Retained S9 state.
pub struct S9State {
    profile: Profile,
    /// Bumped once per coalescing tick that fed anything — the **only** signal
    /// this scenario writes.
    generation: RwSignal<u64>,
    /// Shared with the feed loop: written there, read by the grid widget's
    /// layout. `Rc<RefCell<_>>` rather than a signal precisely so a chunk feed
    /// is not a signal write.
    feed: Rc<RefCell<TerminalFeed>>,
}

impl Component for S9Terminal {
    type State = S9State;

    fn init(&self) -> S9State {
        let profile = resolve_profile();
        let feed = Rc::new(RefCell::new(TerminalFeed::new(profile)));
        let generation = RwSignal::new(0u64);

        // Teardown (a scenario switch) stops the loop at its next wake.
        // `on_cleanup`, never a `Drop` impl — see `docs/CODE_STANDARDS.md`'s
        // State & Reactivity conventions. `Arc<AtomicBool>` rather than
        // `Rc<Cell<_>>` because `on_cleanup` takes a `Send + Sync` closure,
        // even though both ends of this flag live on the UI thread.
        let cancelled = Arc::new(AtomicBool::new(false));
        {
            let cancelled = Arc::clone(&cancelled);
            on_cleanup(move || cancelled.store(true, Ordering::Relaxed));
        }
        spawn_local(run_feed(Rc::clone(&feed), generation, cancelled));

        S9State {
            profile,
            generation,
            feed,
        }
    }

    fn build(&self, state: &mut S9State) -> AnyView<S9State> {
        // Tracked read: the feed loop's generation write wakes this rebuild,
        // and nothing else does.
        let generation = state.generation.get();
        any(Align(
            // Top-left in the safe area (the root already wraps in `safe_area`).
            Alignment::new(-1.0, -1.0),
            TerminalGridView {
                generation,
                profile: state.profile,
                feed: Rc::clone(&state.feed),
            },
        ))
    }
}

// ---------------------------------------------------------------------------
// TerminalGridView / TerminalGridWidget
// ---------------------------------------------------------------------------

/// The declarative grid. See [`TerminalGridWidget`].
pub struct TerminalGridView {
    generation: u64,
    profile: Profile,
    feed: Rc<RefCell<TerminalFeed>>,
}

/// The cell box the grid paints on, and the font size it was derived from.
///
/// Derived from the available content width rather than fixed, so both apps
/// paint an identical, fully visible [`COLS`]x[`ROWS`] grid — see the module
/// docs' width-derived cell geometry section for the defect this exists to fix.
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

/// Solve the cell box that fits exactly [`COLS`]x[`ROWS`] cells into
/// `available`, from a `measure(font_size) -> (cell_advance, line_height)`
/// probe of the engine's own monospace face.
///
/// Pure (the measurement is injected) so the fairness-critical arithmetic is
/// unit-testable without a font, a `TextContext`, or a window. The Dart side
/// runs the same three steps against `ParagraphBuilder`
/// (`flutter_bench/lib/scenarios/s9_terminal.dart`'s `solveCellGeometry`):
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
/// An unbounded axis simply does not constrain; with both unbounded the result
/// is [`FALLBACK_FONT_SIZE`]. The size is floored at [`MIN_FONT_SIZE`], the one
/// case where the returned box may genuinely overflow (see that constant).
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
/// fit, floored at [`MIN_FONT_SIZE`], or [`FALLBACK_FONT_SIZE`] when neither
/// axis is bounded.
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

/// Step 3 of [`solve_cell_geometry`]: how much the current cell box has to
/// shrink to fit `available` (`>= 1.0` means it already fits, within
/// [`FIT_TOLERANCE`]). An unbounded axis never constrains.
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
/// generation, then paints one background rect and one glyph run per batched
/// run.
///
/// Shaping happens in `layout` because that is the only pass with a
/// `TextContext` (`docs/ARCHITECTURE.md`'s Frame pipeline), which is why a new
/// generation must report `ChangeFlags::LAYOUT` — a bare `PAINT` would leave the
/// grid frozen under Android's intra-frame layout-skip gate.
pub struct TerminalGridWidget {
    generation: u64,
    profile: Profile,
    feed: Rc<RefCell<TerminalFeed>>,
    batch: RunBatch,
    /// Which generation `batch` was shaped from (`None` = never).
    shaped: Option<u64>,
    /// The content box the geometry was solved against, and the solution.
    /// Re-solved only when that box changes (a rotation, a density change), and
    /// a re-solve invalidates `shaped` — the runs carry the old font size.
    geometry: Option<(Size, CellGeometry)>,
    // Diagnostics (see `LOG_PREFIX`).
    max_runs: usize,
    painted_frames: u64,
    last_log: Option<FrameTime>,
}

impl<State: 'static> View<State> for TerminalGridView {
    type Element = TerminalGridWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TerminalGridWidget {
        TerminalGridWidget {
            generation: self.generation,
            profile: self.profile,
            feed: Rc::clone(&self.feed),
            batch: RunBatch::default(),
            shaped: None,
            geometry: None,
            max_runs: 0,
            painted_frames: 0,
            last_log: None,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TerminalGridWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.generation == self.generation {
            return ChangeFlags::NONE;
        }
        element.generation = self.generation;
        // LAYOUT, not just PAINT: the reshape happens in `layout`.
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }
}

impl Widget for TerminalGridWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The cell box is derived from the content box, not fixed — the S9
        // fairness fix (see the module docs). Re-solved only when that box
        // changes; a change re-shapes, since every run carries the old size.
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
            // Shape-cache audit line, one per reshape (the shape-cache A/B's
            // second instrument beside `layout_us`). The `frust-perf` prefix
            // rides run.sh's capture whitelist; the `cache` tag keeps it
            // invisible to stats.py's `frust-perf raw` parser and run.sh's
            // raw-frame counter. Counters are cumulative — post-processing
            // diffs consecutive lines for per-window rates.
            if frust_shell_common::perf::enabled() {
                let stats = ctx.text_context::<TextContext>().shape_cache_stats();
                log::info!(
                    "frust-perf cache profile={} gen={} shapes={} line_breaks={} hits={} \
                     evictions={}",
                    self.profile.name(),
                    self.generation,
                    stats.shapes,
                    stats.line_breaks,
                    stats.hits,
                    stats.evictions
                );
            }
        }

        // Exactly COLS x ROWS cells: the constraint that keeps the grid from
        // filling leftover height with extra rows (the Flutter side's
        // `SizedBox` does the same job around `TerminalView`).
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
        // the COLS x ROWS grid can paint" true by construction rather than by
        // arithmetic (the Flutter side's `ClipRect` is its counterpart).
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
        self.log_diagnostics(ctx.frame_time());
        // No `request_frame`: the feed loop's generation write is the sole wake
        // source, which is what lets `idle` settle at zero frames.
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

    /// Emit the once-per-second run-count diagnostic (the primary interpretation
    /// aid for a captured run), gated on `perf::enabled()` so a release/lean
    /// build logs nothing.
    fn log_diagnostics(&mut self, now: FrameTime) {
        if !frust_shell_common::perf::enabled() {
            return;
        }
        let due = match self.last_log {
            Some(last) => now.saturating_sub(last) >= LOG_INTERVAL,
            None => true,
        };
        if !due {
            return;
        }
        self.last_log = Some(now);
        let feed = self.feed.borrow();
        // The geometry fields are the fairness gate's audit trail: the Flutter
        // side logs the same keys (`bench-s9 ... font_size= cell_w= cell_h=
        // cols= rows= grid_w= grid_h= avail_w= avail_h=`), so a capture proves
        // the two apps painted the same grid instead of leaving it assumed.
        let font_size = self
            .geometry
            .map_or(f64::NAN, |(_, g)| f64::from(g.font_size));
        let cell_width = self.geometry.map_or(f64::NAN, |(_, g)| g.width);
        let cell_height = self.geometry.map_or(f64::NAN, |(_, g)| g.height);
        // The content box it was all derived from: if the two shells ever report
        // different logical sizes for the same screen, this is the field that
        // shows it rather than leaving a font-size mismatch unexplained.
        let available = self.geometry.map_or(Size::ZERO, |(a, _)| a);
        log::info!(
            "{LOG_PREFIX} profile={} gen={} runs={} max_runs={} frames={} chunks={}/{} bytes={} \
             font_size={font_size:.4} cell_w={cell_width:.4} cell_h={cell_height:.4} \
             cols={COLS} rows={ROWS} grid_w={:.2} grid_h={:.2} \
             avail_w={:.2} avail_h={:.2}",
            self.profile.name(),
            self.generation,
            self.batch.count(),
            self.max_runs,
            self.painted_frames,
            feed.fed_chunks(),
            self.profile.chunk_count(),
            feed.fed_bytes(),
            cell_width * f64::from(COLS),
            cell_height * f64::from(ROWS),
            available.width,
            available.height,
        );
    }
}

/// Measure the monospace cell box at `font_size`: shape [`CELL_PROBE`] and
/// divide its advance by its length (its height is the shaped line box,
/// [`LINE_HEIGHT`] x `font_size`). The probe [`solve_cell_geometry`] solves
/// against — never an assumed advance ratio.
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
    use frust_core::FrameTime;
    use kurbo::BezPath;
    use peniko::Brush;
    use std::any::Any;

    // -----------------------------------------------------------------
    // Fixture integrity (the fairness gate: both apps replay these bytes)
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

    // -----------------------------------------------------------------
    // Run batching (research finding 2)
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
    // Run counts per profile — the primary diagnostic (finding 3)
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
    // Profile selection
    // -----------------------------------------------------------------

    #[test]
    fn profile_names_round_trip_case_insensitively() {
        for profile in Profile::ALL {
            assert_eq!(Profile::from_name(profile.name()), Some(profile));
            assert_eq!(
                Profile::from_name(&profile.name().to_uppercase()),
                Some(profile)
            );
        }
        assert_eq!(Profile::from_name("nope"), None);
        assert_eq!(Profile::from_name(""), None);
    }

    #[test]
    fn deep_link_path_segment_selects_a_profile() {
        assert_eq!(
            profile_from_url("frustbench://s9/htop"),
            Some(Profile::Htop)
        );
        assert_eq!(
            profile_from_url("frustbench://s9/build-log?x=1"),
            Some(Profile::BuildLog)
        );
        // Bare `s9` (no path) keeps the default resolution chain going.
        assert_eq!(profile_from_url("frustbench://s9"), None);
        assert_eq!(profile_from_url("frustbench://s9/"), None);
        // Another scenario's link must never retarget S9.
        assert_eq!(profile_from_url("frustbench://s3/htop"), None);
        assert_eq!(profile_from_url("other://s9/htop"), None);
        assert_eq!(profile_from_url("frustbench://s9/nope"), None);
    }

    #[test]
    fn every_profile_has_a_distinct_static_sub_marker() {
        let mut seen = Vec::new();
        for profile in Profile::ALL {
            let m = profile_marker(profile);
            assert!(m.starts_with("s9-"), "{m}");
            assert!(!seen.contains(&m), "duplicate sub-marker {m}");
            seen.push(m);
        }
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
    // Width-derived cell geometry (the S9 fairness fix)
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
        // The OP9's logical safe-area box (1080x2400 at ~2.75 density).
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
        // 0.6em (the two apps' faces differ: 0.6 vs 0.60009765625).
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
        // A hypothetical face that rounds every advance up to a whole pixel:
        // the ratio-derived starting size overflows, so the shrink pass must
        // bring it back inside the box (this is why the solver re-measures).
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
        // 80 columns cannot fit 100 logical px at any sane size. The solver
        // floors rather than shipping a 2px font; the module docs say such a run
        // is not comparable (the fix is a narrower grid in BOTH apps).
        let geometry = solve_cell_geometry(Size::new(100.0, 872.7), linear_face(0.6));
        assert_eq!(geometry.font_size, MIN_FONT_SIZE);
        assert!(
            geometry.grid_width() > 100.0,
            "the floor is knowingly wider than the box: {geometry:?}"
        );
    }

    // -----------------------------------------------------------------
    // Widget layout/paint smoke test (no GPU, no window)
    // -----------------------------------------------------------------

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        glyph_runs: usize,
        clips: usize,
        lines: usize,
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
        fn stroke_line(&mut self, _p0: Point, _p1: Point, _w: f64, _c: Color) {
            self.lines += 1;
        }
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
            profile,
            feed: Rc::new(RefCell::new(feed)),
        };
        let mut counter = 0u64;
        let widget = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        (widget, expected)
    }

    #[test]
    fn grid_shapes_one_glyph_run_per_batched_run_and_clips_itself() {
        let (mut widget, expected) = grid_at(Profile::BuildLog, 60);
        assert!(expected > 0);

        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let available = Size::new(392.7, 872.7); // the OP9's logical safe-area box
        let size = widget.layout(&mut lctx, &BoxConstraints::loose(available));
        // The grid is exactly COLS x ROWS cells, and it fits the content box:
        // no clipped column, no row beyond the 45th (the S9 fairness fix).
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
    }

    #[test]
    fn idle_profile_paints_only_the_backdrop_and_requests_nothing() {
        let (mut widget, expected) = grid_at(Profile::Idle, 0);
        assert_eq!(expected, 0);

        let mut tcx = TextContext::new();
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
        let make = |generation: u64| TerminalGridView {
            generation,
            profile: Profile::BuildLog,
            feed: Rc::clone(&feed),
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
    fn reshaping_is_skipped_while_the_generation_is_unchanged() {
        let (mut widget, _) = grid_at(Profile::BuildLog, 60);
        let mut tcx = TextContext::new();
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
        let mut tcx = TextContext::new();

        let mut layout_at = |available: Size, widget: &mut TerminalGridWidget| {
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            widget.layout(&mut lctx, &BoxConstraints::loose(available))
        };

        // A real monospace face (whatever the host resolves for the
        // `Monospace` generic slot) must fill the width to within one cell.
        let phone = Size::new(392.7, 872.7);
        let size = layout_at(phone, &mut widget);
        let (_, geometry) = widget.geometry.expect("solved");
        assert!(size.width <= phone.width + 0.01, "{size:?}");
        assert!(
            phone.width - size.width < geometry.width,
            "less than one cell of unused width: {size:?} at {geometry:?}"
        );

        // A wider box (rotation / a resized desktop window) re-solves rather
        // than keeping the old cell size, and re-shapes at the new size.
        let wide = Size::new(700.0, 872.7);
        let resized = layout_at(wide, &mut widget);
        let (_, resized_geometry) = widget.geometry.expect("re-solved");
        assert!(resized.width > size.width, "{resized:?} vs {size:?}");
        assert!(resized_geometry.font_size > geometry.font_size);
        assert!(resized.width <= wide.width + 0.01, "{resized:?}");
        assert!(resized.height <= wide.height + 0.01, "{resized:?}");
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

    #[test]
    fn frame_time_log_gate_is_rate_limited_to_one_second() {
        // `perf::enabled()` is off in a plain `cargo test` (no `perf-trace`
        // feature, no FRUST_TRACE), so this asserts the gate, not the output.
        let (mut widget, _) = grid_at(Profile::BuildLog, 1);
        widget.log_diagnostics(FrameTime::from_nanos(0));
        assert!(
            widget.last_log.is_none() || frust_shell_common::perf::enabled(),
            "a perf-disabled build must not even stamp the log clock"
        );
    }
}
