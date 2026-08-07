//! `rows_profile` — fixture changed-rows analysis for row-scoped terminal reshaping.
//!
//! Host-only, standalone measurement: replays the checked-in S9 terminal fixtures
//! (`benchmarks/harness/fixtures/terminal/{typing,build-log,htop}.{chunks,json}`) through a
//! real `vt100::Parser`, generation by generation, using the EXACT chunk-to-generation
//! grouping `s9_terminal.rs`'s feed loop uses, and reports the changed-rows-per-generation
//! distribution plus the shaping-cost saving a row-scoped reshape would realize — the
//! GO/NO-GO input for the pivot from run-granular to row-granular shape caching in the
//! terminal widget (does a generation touch few enough rows for row scoping to pay?).
//!
//! Deliberately **std + `vt100` only** (no `serde_json`, no path back into `frustbench`
//! itself): this is a throwaway measurement tool, not shipped/gated code (an example binary
//! is never built by `cargo build`/`test --workspace`, and `frust_bench` is itself a
//! standalone workspace excluded from the frust root graph — see `docs/DEVELOPMENT.md`'s
//! Benchmarks section). Because it stands alone, it REIMPLEMENTS rather than imports the two
//! rules it must mirror faithfully — the feed's generation grouping and `batch_screen`'s run
//! batching — each cited below against `s9_terminal.rs`'s exact line range so a reviewer can
//! check the mirror without reading two files side by side.
//!
//! Run from the bench workspace:
//!
//! ```text
//! cd benchmarks/frust_bench && cargo run --example rows_profile
//! ```

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Geometry + cadence constants — mirror `s9_terminal.rs`'s (frust_bench's OWN 80x45 S9
// grid, not muxr's size-derived one; see the fixture README's "Grid is fixed" note).
// ---------------------------------------------------------------------------

/// Mirrors `s9_terminal.rs::COLS` (`s9_terminal.rs:251`).
const COLS: u16 = 80;
/// Mirrors `s9_terminal.rs::ROWS` (`s9_terminal.rs:253`).
const ROWS: u16 = 45;

/// The feed loop's repaint cap in Hz — the one knob the grouping rule below derives from.
/// Mirrors `s9_terminal.rs::REPAINT_HZ` (`s9_terminal.rs:310`).
const REPAINT_HZ: f64 = 30.0;
/// One repaint tick, in ms (derived, matching `s9_terminal.rs::REPAINT_INTERVAL`,
/// `s9_terminal.rs:315` — computed in plain ms here since this offline replay has no real
/// clock to nanosecond-align a `Duration` to).
const REPAINT_INTERVAL_MS: f64 = 1000.0 / REPAINT_HZ;

/// Default background — mirrors `s9_terminal.rs::DEFAULT_BG` (`s9_terminal.rs:349`).
const DEFAULT_BG: (u8, u8, u8) = (0x10, 0x10, 0x10);
/// Default foreground — mirrors `s9_terminal.rs::DEFAULT_FG` (`s9_terminal.rs:347`), needed
/// only because `resolve`'s inverse swap can hand the foreground into the background slot.
const DEFAULT_FG: (u8, u8, u8) = (0xD0, 0xD0, 0xD0);

/// The ANSI 16-color table. Mirrors `s9_terminal.rs::ANSI_16` (`s9_terminal.rs:353-370`);
/// ported in full even though the fixture subset (`gen_terminal_fixtures.py`'s `FG`/`BG`
/// lists: SGR 30-37/40-47 only) never reaches past index 7, so an out-of-subset byte still
/// resolves rather than panicking.
const ANSI_16: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0xCD, 0x00, 0x00),
    (0x00, 0xCD, 0x00),
    (0xCD, 0xCD, 0x00),
    (0x00, 0x00, 0xEE),
    (0xCD, 0x00, 0xCD),
    (0x00, 0xCD, 0xCD),
    (0xE5, 0xE5, 0xE5),
    (0x7F, 0x7F, 0x7F),
    (0xFF, 0x00, 0x00),
    (0x00, 0xFF, 0x00),
    (0xFF, 0xFF, 0x00),
    (0x5C, 0x5C, 0xFF),
    (0xFF, 0x00, 0xFF),
    (0x00, 0xFF, 0xFF),
    (0xFF, 0xFF, 0xFF),
];

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

/// The three fixtures this task measures (`typing`/`build-log`/`htop` — per the task's Inputs
/// list; `idle` has no chunks and `firehose` isn't in scope for the row-scoped-reshape
/// decision).
const PROFILES: [&str; 3] = ["typing", "build-log", "htop"];

/// Resolves a fixture path from the crate root, independent of the process's cwd (`cargo run
/// --example` from `benchmarks/frust_bench/` and a bare invocation from elsewhere both work).
fn fixture_path(name: &str, ext: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../harness/fixtures/terminal")
        .join(format!("{name}.{ext}"))
}

/// Pulls a bare numeric field (`"key": 123`) out of a fixture's flat `<profile>.json`
/// manifest (format: `benchmarks/harness/fixtures/terminal/README.md`'s Replay contract).
/// Not a general JSON parser — the manifest is a fixed flat object of string/number fields
/// written by `gen_terminal_fixtures.py`, so a targeted scan is enough and avoids adding a
/// `serde_json` dependency to a std+vt100-only tool.
fn json_u64_field(json: &str, key: &str) -> u64 {
    let needle = format!("\"{key}\"");
    let key_at = json
        .find(&needle)
        .unwrap_or_else(|| panic!("manifest field {key:?} not found"));
    let after_key = &json[key_at + needle.len()..];
    let colon_at = after_key
        .find(':')
        .unwrap_or_else(|| panic!("manifest field {key:?} has no ':'"));
    let value = after_key[colon_at + 1..].trim_start();
    let digits_len = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    value[..digits_len]
        .parse()
        .unwrap_or_else(|_| panic!("manifest field {key:?} is not a plain integer"))
}

/// Splits a `.chunks` file into its length-prefixed records. Mirrors
/// `s9_terminal.rs::next_chunk`/`walk_chunks` (`s9_terminal.rs:564-584`): `repeat [u32 LE
/// length][length bytes]`, stopping cleanly (never panicking) on a truncated tail.
fn split_chunks(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(payload) = next_chunk(bytes, &mut cursor) {
        out.push(payload);
    }
    out
}

/// Reads one `[u32 LE length][length bytes]` record at `cursor`, advancing it past the
/// payload. `None` on a clean end **or** a truncated tail — a truncated fixture stops the
/// replay rather than panicking, mirroring `s9_terminal.rs::next_chunk` (`s9_terminal.rs:564-572`).
fn next_chunk<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let header_end = cursor.checked_add(4)?;
    let header = bytes.get(*cursor..header_end)?;
    let len = u32::from_le_bytes(header.try_into().expect("4-byte slice")) as usize;
    let payload_end = header_end.checked_add(len)?;
    let payload = bytes.get(header_end..payload_end)?;
    *cursor = payload_end;
    Some(payload)
}

// ---------------------------------------------------------------------------
// Generation grouping — the replay-faithfulness-critical half
// ---------------------------------------------------------------------------

/// Generation boundaries under the feed loop's exact coalescing rule.
///
/// Mirrors `run_feed`/`TerminalFeed::feed_due` (`s9_terminal.rs:954-984`, `s9_terminal.rs:630-657`):
/// the feed wakes on an ideal (jitter-free) `REPAINT_INTERVAL` grid — wake `k` lands at
/// simulated time `t_k = k * REPAINT_INTERVAL_MS` (`s9_terminal.rs:978-979`'s `let deadline =
/// REPAINT_INTERVAL * tick;`, assuming the sleep hits its deadline exactly, which is the only
/// assumption an offline replay with no real clock CAN make) — and at each wake feeds every
/// chunk `i` whose schedule `i * interval_ms` is due (`s9_terminal.rs:638`'s break condition,
/// `if self.next_index as f64 * interval_ms > elapsed_ms { break; }`), publishing exactly ONE
/// generation per wake that fed at least one chunk (`s9_terminal.rs:966-972`: `let fed =
/// feed.borrow_mut().feed_due(...); if fed > 0 { published += 1; generation.set(published); }`).
/// A wake that fed zero chunks publishes nothing and is invisible to this replay, exactly as
/// it is invisible to the app (no signal write, no repaint that generation).
///
/// `interval_ms` mirrors `Profile::interval_ms` (`s9_terminal.rs:487-492`: `1000.0 /
/// f64::from(hz)`). Returns one entry per PUBLISHED generation: the chunk indices fed at that
/// wake, in order — so `firehose`-rate profiles (faster than `REPAINT_HZ`) would batch several
/// chunks per generation, and `typing`-rate profiles (slower) would skip several silent wakes
/// between generations. Neither of the three profiles this task measures needs multi-chunk
/// batching in practice (`build-log`'s 30 Hz matches `REPAINT_HZ` almost exactly, `htop`'s
/// 10 Hz and `typing`'s 5 Hz are both slower) — verified in the printed report below, not
/// assumed.
fn group_into_generations(chunk_count: usize, hz: u64) -> Vec<Vec<usize>> {
    let interval_ms = 1000.0 / hz as f64;
    let mut generations = Vec::new();
    let mut next_index = 0usize;
    let mut tick = 0u64;
    while next_index < chunk_count {
        let elapsed_ms = tick as f64 * REPAINT_INTERVAL_MS;
        let mut fed = Vec::new();
        while next_index < chunk_count {
            if next_index as f64 * interval_ms > elapsed_ms {
                break;
            }
            fed.push(next_index);
            next_index += 1;
        }
        if !fed.is_empty() {
            generations.push(fed);
        }
        tick += 1;
    }
    generations
}

// ---------------------------------------------------------------------------
// Style key + run batching — mirrors `s9_terminal.rs`'s `CellStyle`/`batch_screen`
// ---------------------------------------------------------------------------

/// Mirrors `s9_terminal.rs::CellStyle` (`s9_terminal.rs:684-692`): everything about a cell
/// that affects which run it batches into. Two consecutive cells batch into one run iff their
/// whole `CellStyle` matches — the kterm-parity contract `batch_screen` implements.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct CellStyle {
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    dim: bool,
}

impl CellStyle {
    /// Mirrors `s9_terminal.rs::CellStyle::from_cell` (`s9_terminal.rs:695-705`).
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

    /// Whether this style's resolved background equals the terminal default — the half of
    /// `s9_terminal.rs::CellStyle::resolve` (`s9_terminal.rs:707-723`) run-survival needs (the
    /// fg/dim halves only affect paint color, never whether a run is discarded).
    fn bg_is_default(self) -> bool {
        let fg = resolve_color(self.fg, DEFAULT_FG);
        let bg = resolve_color(self.bg, DEFAULT_BG);
        let bg = if self.inverse { fg } else { bg };
        bg == DEFAULT_BG
    }
}

/// Mirrors `s9_terminal.rs::resolve_color` (`s9_terminal.rs:752-758`).
fn resolve_color(color: vt100::Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        vt100::Color::Default => fallback,
        vt100::Color::Idx(i) => indexed_color(i),
        vt100::Color::Rgb(r, g, b) => (r, g, b),
    }
}

/// Mirrors `s9_terminal.rs::indexed_color` (`s9_terminal.rs:764-781`).
fn indexed_color(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => ANSI_16[i as usize],
        16..=231 => {
            const LEVELS: [u8; 6] = [0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF];
            let v = i - 16;
            (
                LEVELS[(v / 36) as usize],
                LEVELS[((v % 36) / 6) as usize],
                LEVELS[(v % 6) as usize],
            )
        }
        232..=255 => {
            let g = 8 + (i - 232) * 10;
            (g, g, g)
        }
    }
}

/// Counts row `row`'s batched runs under `batch_screen`'s exact rule
/// (`s9_terminal.rs:910-942` walking the row; `commit` at `s9_terminal.rs:884-898` deciding
/// whether a run survives): consecutive cells sharing a whole `CellStyle` coalesce into one
/// run; a wide-continuation cell contributes nothing of its own
/// (`s9_terminal.rs:916-918` — never exercised by these fixtures, whose subset is pure ASCII,
/// per the fixture README, but mirrored for fidelity); a run whose resolved background is the
/// terminal default is trimmed of trailing blanks and DISCARDED entirely if every cell in it
/// was blank (`s9_terminal.rs:889-896` — `trim_end_matches(' ')` leaves an empty string iff
/// the whole run was blank, since trimming only removes from the end); a run with a
/// non-default background always survives, even if blank, because its rect still paints.
fn count_row_runs(screen: &vt100::Screen, row: u16) -> usize {
    let mut count = 0usize;
    let mut open: Option<CellStyle> = None;
    let mut run_all_blank = true;
    for col in 0..COLS {
        let cell = screen.cell(row, col);
        if cell.is_some_and(vt100::Cell::is_wide_continuation) {
            continue;
        }
        let style = cell.map(CellStyle::from_cell).unwrap_or_default();
        let is_blank = match cell {
            Some(c) if c.has_contents() => c.contents() == " ",
            _ => true,
        };
        if open != Some(style) {
            if let Some(prev) = open
                && run_survives(prev, run_all_blank)
            {
                count += 1;
            }
            open = Some(style);
            run_all_blank = true;
        }
        if !is_blank {
            run_all_blank = false;
        }
    }
    if let Some(prev) = open
        && run_survives(prev, run_all_blank)
    {
        count += 1;
    }
    count
}

/// A run's `commit()` survival test — see `count_row_runs`'s doc for the cited rule.
fn run_survives(style: CellStyle, all_blank: bool) -> bool {
    if style.bg_is_default() {
        !all_blank
    } else {
        true
    }
}

// ---------------------------------------------------------------------------
// Row-changed detection — an app-owned snapshot (NOT a `Screen` clone; NOT `rows_diff`)
// ---------------------------------------------------------------------------

/// An app-owned copy of the visible grid's cells, retained across generations so the CURRENT
/// generation's live `Screen` can be compared against it without ever cloning a whole
/// `Screen` (which would also copy the scrollback in the real app).
fn snapshot_rows(screen: &vt100::Screen) -> Vec<Vec<vt100::Cell>> {
    (0..ROWS)
        .map(|row| {
            (0..COLS)
                .map(|col| {
                    screen
                        .cell(row, col)
                        .cloned()
                        .expect("cell in bounds of the fixed 80x45 grid")
                })
                .collect()
        })
        .collect()
}

/// The app-style cell compare: `Screen::cell(row, col)` (returning
/// `Option<&Cell>`) against the retained snapshot's `Option<&Cell>`, using `Cell: PartialEq`
/// directly — deliberately NOT `vt100::Screen::rows_diff`, which builds an escape-byte
/// `Vec<u8>` per row by comparing every cell anyway: a direct compare has the same cost with
/// none of the byte-building, and the eventual row-scoped design won't use it either.
fn row_changed(screen: &vt100::Screen, row: u16, prev_row: &[vt100::Cell]) -> bool {
    (0..COLS).any(|col| screen.cell(row, col) != prev_row.get(col as usize))
}

// ---------------------------------------------------------------------------
// Per-profile analysis
// ---------------------------------------------------------------------------

struct ProfileReport {
    name: &'static str,
    generations: usize,
    /// Raw (content-only) changed-row count per generation — the distribution the GO/NO-GO
    /// rubric's median/p90/max apply to.
    changed_counts: Vec<usize>,
    /// Total batched runs (summed over all 45 rows) per generation.
    total_runs_series: Vec<usize>,
    /// Sum, across every generation, of runs sitting in rows unchanged from the PRIOR
    /// generation (content-only — no cursor adjustment). The "raw saving" numerator.
    raw_skippable_sum: u64,
    /// Same, but a row is also excluded (not counted as skippable) if it is the cursor's
    /// previous OR current row that generation (the cursor carve-out: a caret move reshapes
    /// both of those rows even when their content is identical). The "effective saving"
    /// numerator.
    effective_skippable_sum: u64,
    /// Sum, across every generation, of total runs — the shared denominator for both
    /// percentages above.
    total_runs_sum: u64,
}

fn analyze_profile(name: &'static str) -> ProfileReport {
    let json_path = fixture_path(name, "json");
    let json =
        std::fs::read_to_string(&json_path).unwrap_or_else(|e| panic!("read {json_path:?}: {e}"));
    let hz = json_u64_field(&json, "hz");
    let manifest_chunk_count = json_u64_field(&json, "chunk_count");
    let manifest_cols = json_u64_field(&json, "cols");
    let manifest_rows = json_u64_field(&json, "rows");
    assert_eq!(
        manifest_cols,
        u64::from(COLS),
        "{name}: manifest cols mismatch"
    );
    assert_eq!(
        manifest_rows,
        u64::from(ROWS),
        "{name}: manifest rows mismatch"
    );

    let chunks_path = fixture_path(name, "chunks");
    let bytes = std::fs::read(&chunks_path).unwrap_or_else(|e| panic!("read {chunks_path:?}: {e}"));
    let chunks = split_chunks(&bytes);
    assert_eq!(
        chunks.len() as u64,
        manifest_chunk_count,
        "{name}: fixture integrity — walked chunk count doesn't match the manifest \
         (mirrors s9_terminal.rs's own walk_chunks integrity gate)"
    );

    let generations = group_into_generations(chunks.len(), hz);

    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    let mut prev_rows = snapshot_rows(parser.screen());
    let mut prev_cursor_row = parser.screen().cursor_position().0;

    let mut changed_counts = Vec::with_capacity(generations.len());
    let mut total_runs_series = Vec::with_capacity(generations.len());
    let mut raw_skippable_sum = 0u64;
    let mut effective_skippable_sum = 0u64;
    let mut total_runs_sum = 0u64;

    for indices in &generations {
        for &i in indices {
            parser.process(chunks[i]);
        }
        let screen = parser.screen();
        let cur_cursor_row = screen.cursor_position().0;

        let mut row_runs = [0usize; ROWS as usize];
        let mut changed = [false; ROWS as usize];
        for row in 0..ROWS {
            row_runs[row as usize] = count_row_runs(screen, row);
            changed[row as usize] = row_changed(screen, row, &prev_rows[row as usize]);
        }

        let changed_row_count = changed.iter().filter(|&&c| c).count();
        let total_runs: usize = row_runs.iter().sum();

        let mut raw_skippable = 0usize;
        let mut effective_skippable = 0usize;
        for row in 0..ROWS as usize {
            if changed[row] {
                continue;
            }
            raw_skippable += row_runs[row];
            let forced = row as u16 == prev_cursor_row || row as u16 == cur_cursor_row;
            if !forced {
                effective_skippable += row_runs[row];
            }
        }

        changed_counts.push(changed_row_count);
        total_runs_series.push(total_runs);
        raw_skippable_sum += raw_skippable as u64;
        effective_skippable_sum += effective_skippable as u64;
        total_runs_sum += total_runs as u64;

        prev_rows = snapshot_rows(screen);
        prev_cursor_row = cur_cursor_row;
    }

    ProfileReport {
        name,
        generations: changed_counts.len(),
        changed_counts,
        total_runs_series,
        raw_skippable_sum,
        effective_skippable_sum,
        total_runs_sum,
    }
}

// ---------------------------------------------------------------------------
// Stats + report printing
// ---------------------------------------------------------------------------

/// Nearest-rank percentile over an already-sorted slice.
fn percentile(sorted: &[usize], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Ten `ROWS`-wide buckets (`[0-4] [5-9] ... [40-44] [45-49]`, the last holding only the
/// all-rows-changed case) over the changed-rows-per-generation series.
fn histogram(counts: &[usize]) -> [usize; 10] {
    let mut buckets = [0usize; 10];
    for &c in counts {
        buckets[(c / 5).min(9)] += 1;
    }
    buckets
}

fn print_report(r: &ProfileReport) {
    let mut sorted_changed = r.changed_counts.clone();
    sorted_changed.sort_unstable();
    let median = percentile(&sorted_changed, 50.0);
    let p90 = percentile(&sorted_changed, 90.0);
    let max = *sorted_changed.last().unwrap_or(&0);
    let hist = histogram(&r.changed_counts);

    let mut sorted_runs = r.total_runs_series.clone();
    sorted_runs.sort_unstable();
    let runs_median = percentile(&sorted_runs, 50.0);
    let runs_min = *sorted_runs.first().unwrap_or(&0);
    let runs_max = *sorted_runs.last().unwrap_or(&0);

    let raw_pct = if r.total_runs_sum > 0 {
        100.0 * r.raw_skippable_sum as f64 / r.total_runs_sum as f64
    } else {
        0.0
    };
    let eff_pct = if r.total_runs_sum > 0 {
        100.0 * r.effective_skippable_sum as f64 / r.total_runs_sum as f64
    } else {
        0.0
    };

    println!("=== {} ===", r.name);
    println!("generations: {}", r.generations);
    println!("changed-rows/gen (of {ROWS}): median={median} p90={p90} max={max}");
    print!("histogram (changed-rows bucket:generation-count):");
    for (i, count) in hist.iter().enumerate() {
        let lo = i * 5;
        let hi = lo + 4;
        print!(" [{lo}-{hi}]:{count}");
    }
    println!();
    println!("total runs/gen: median={runs_median} min={runs_min} max={runs_max}");
    println!(
        "raw saving (runs in unchanged rows / total runs): {raw_pct:.1}%  ({}/{})",
        r.raw_skippable_sum, r.total_runs_sum
    );
    println!(
        "effective saving (raw, minus rows forced dirty by cursor prev+cur): {eff_pct:.1}%  ({}/{})",
        r.effective_skippable_sum, r.total_runs_sum
    );
    println!();
}

fn main() {
    for &name in &PROFILES {
        let report = analyze_profile(name);
        print_report(&report);
    }
}
