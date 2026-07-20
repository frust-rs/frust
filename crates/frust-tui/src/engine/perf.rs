//! Perf-line parsing + the per-session sparkline panel (PLAN.md D5/D6 —
//! "the unique Frust advantage"): tolerant parsing of `frust-perf` log lines
//! (`frust-shell-common::perf`'s summary/raw/startup line grammar, also
//! documented in `benchmarks/PROTOCOL.md`) into a bounded per-session ring of
//! recent frame timings plus the latest summary/startup snapshot.
//!
//! Parsing never panics on a malformed line — every field is `.parse().ok()`,
//! and a line missing/mangling a required field just isn't a perf line
//! (`None`), left in the log unmodified. See the fixture tests below for the
//! exact grammar accepted. **Callers must pass an already ANSI-stripped
//! line** (`super::session_view::strip_ansi`) — a raw line carrying its own
//! color codes never matches the `frust-perf ` prefix.

use std::collections::{HashMap, VecDeque};

/// Ring capacity for [`PerfPanel`]'s sparkline — aligned by value with
/// `frust-shell-common::perf::RING_CAPACITY` (the shell's own per-frame
/// ring), so the TUI sparkline shows roughly the same "recent window" the
/// shell itself aggregates over. That const is private to that crate, so
/// this is a deliberate by-value alignment (the `SessionView::LOG_LINE_CAP`
/// precedent), not a shared symbol.
const FRAME_RING_CAP: usize = 120;

/// One parsed `frust-perf raw` line (format v2 — see
/// `benchmarks/PROTOCOL.md`'s "Frust: `frust-perf raw`" section and
/// `frust-shell-common::perf::format_raw_frame_line`'s doc comment for the
/// v1→v2 `encode_us`/`present_us` field-split history). Only emitted when
/// both `FRUST_TRACE` and `FRUST_TRACE_RAW` are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFrame {
    /// 1-indexed running frame counter.
    pub n: u64,
    /// Whole-frame total, microseconds.
    pub total_us: u64,
    /// Rebuild-pass duration, microseconds.
    pub rebuild_us: u64,
    /// Layout-pass duration, microseconds.
    pub layout_us: u64,
    /// Paint-pass duration, microseconds.
    pub paint_us: u64,
    /// Encode-pass duration, microseconds.
    pub encode_us: u64,
    /// Present-pass duration, microseconds.
    pub present_us: u64,
    /// Whether the frame-gate skipped this frame.
    pub skipped: bool,
}

/// One parsed `frust-perf frame` periodic-summary line — emitted roughly
/// every ~2s of frames regardless of `FRUST_TRACE_RAW`
/// (`frust-shell-common::perf::FrameStats::emit_log`), so this is the
/// "always present once `FRUST_TRACE` is on" half of the panel's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSummary {
    /// The frame count this summary window covers.
    pub n: u64,
    /// Median whole-frame total, milliseconds.
    pub total_p50_ms: u64,
    /// 95th-percentile whole-frame total, milliseconds.
    pub total_p95_ms: u64,
    /// 99th-percentile whole-frame total, milliseconds.
    pub total_p99_ms: u64,
    /// Frames the frame-gate skipped in this window.
    pub skipped: u64,
    /// Lifetime running frame counter.
    pub total_frames: u64,
}

/// One parsed `frust-perf startup` line: every recorded milestone
/// name→millisecond pair, in the order the line carries them
/// (`frust-shell-common::perf::StartupSpans::emit_log`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupSummary {
    /// `(span name, delta from the startup epoch in ms)`, insertion order.
    pub spans: Vec<(String, u64)>,
}

/// A successfully-parsed `frust-perf` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerfLine {
    /// A per-frame raw sample (`FRUST_TRACE_RAW` only).
    Raw(RawFrame),
    /// A periodic percentile summary (always, once `FRUST_TRACE` is on).
    Frame(FrameSummary),
    /// The one-shot cold-start span line.
    Startup(StartupSummary),
}

/// Parse one already-ANSI-stripped log line as a `frust-perf` line, or
/// `None` if it isn't one — tolerant of any other line in the stream (a
/// build/run tool's own output) and never panics on a malformed
/// `frust-perf`-prefixed line either: a missing/garbled required field just
/// yields `None`, so the line is silently not treated as perf data rather
/// than crashing the session's log parse.
pub fn parse_perf_line(line: &str) -> Option<PerfLine> {
    let rest = line.trim().strip_prefix("frust-perf ")?;
    let (kind, fields) = rest.split_once(' ').unwrap_or((rest, ""));
    match kind {
        "raw" => parse_raw(fields).map(PerfLine::Raw),
        "frame" => parse_frame(fields).map(PerfLine::Frame),
        "startup" => Some(PerfLine::Startup(parse_startup(fields))),
        _ => None,
    }
}

/// Split space-separated `key=value` tokens into a lookup map — shared by
/// [`parse_raw`]/[`parse_frame`]. A token with no `=` is silently dropped
/// (tolerant parsing, never a hard error).
fn field_map(fields: &str) -> HashMap<&str, &str> {
    fields
        .split_whitespace()
        .filter_map(|tok| tok.split_once('='))
        .collect()
}

fn parse_u64(map: &HashMap<&str, &str>, key: &str) -> Option<u64> {
    map.get(key)?.parse().ok()
}

fn parse_raw(fields: &str) -> Option<RawFrame> {
    let map = field_map(fields);
    Some(RawFrame {
        n: parse_u64(&map, "n")?,
        total_us: parse_u64(&map, "total_us")?,
        rebuild_us: parse_u64(&map, "rebuild_us")?,
        layout_us: parse_u64(&map, "layout_us")?,
        paint_us: parse_u64(&map, "paint_us")?,
        encode_us: parse_u64(&map, "encode_us")?,
        present_us: parse_u64(&map, "present_us")?,
        skipped: parse_u64(&map, "skipped")? != 0,
    })
}

fn parse_frame(fields: &str) -> Option<FrameSummary> {
    let map = field_map(fields);
    Some(FrameSummary {
        n: parse_u64(&map, "n")?,
        total_p50_ms: parse_u64(&map, "total_p50_ms")?,
        total_p95_ms: parse_u64(&map, "total_p95_ms")?,
        total_p99_ms: parse_u64(&map, "total_p99_ms")?,
        skipped: parse_u64(&map, "skipped")?,
        total_frames: parse_u64(&map, "total_frames")?,
    })
}

/// A startup line's spans are unbounded/free-form (`name=Xms` per recorded
/// milestone) — parsed permissively: a malformed token (no `=`, or a
/// non-numeric/non-`ms`-suffixed value) is skipped rather than failing the
/// whole line, so one bad span never hides every other one.
fn parse_startup(fields: &str) -> StartupSummary {
    let spans = fields
        .split_whitespace()
        .filter_map(|tok| {
            let (name, val) = tok.split_once('=')?;
            let ms = val.strip_suffix("ms")?.parse::<u64>().ok()?;
            Some((name.to_string(), ms))
        })
        .collect();
    StartupSummary { spans }
}

/// The per-session perf panel state (PLAN.md D5/D6): a bounded ring of
/// recent per-frame totals (microseconds, from `frust-perf raw` lines —
/// only ever populated when `FRUST_TRACE_RAW` is on), plus the latest
/// periodic summary/startup line seen (the summary is always present once
/// `FRUST_TRACE` is on, regardless of the raw dial). [`Self::has_data`]
/// gates the panel's zero-noise render contract: it shows nothing until at
/// least one `frust-perf` line has actually appeared in the session's
/// output.
#[derive(Debug, Clone, Default)]
pub struct PerfPanel {
    /// Whether the panel is expanded for its session tab (`t` toggles it).
    pub visible: bool,
    ring: VecDeque<u64>,
    /// The latest `frust-perf frame` summary seen, if any.
    pub last_frame: Option<FrameSummary>,
    /// The latest `frust-perf startup` line seen, if any (emitted once, near
    /// the start of a session's output).
    pub last_startup: Option<StartupSummary>,
}

impl PerfPanel {
    /// Feed one already-ANSI-stripped log line; a no-op for anything that
    /// isn't a recognized `frust-perf` line (see [`parse_perf_line`]).
    pub fn ingest(&mut self, line: &str) {
        match parse_perf_line(line) {
            Some(PerfLine::Raw(frame)) => {
                self.ring.push_back(frame.total_us);
                if self.ring.len() > FRAME_RING_CAP {
                    self.ring.pop_front();
                }
            }
            Some(PerfLine::Frame(summary)) => self.last_frame = Some(summary),
            Some(PerfLine::Startup(summary)) => self.last_startup = Some(summary),
            None => {}
        }
    }

    /// Whether any `frust-perf` line has appeared yet — the panel's
    /// zero-noise render gate.
    pub fn has_data(&self) -> bool {
        !self.ring.is_empty() || self.last_frame.is_some() || self.last_startup.is_some()
    }

    /// The retained per-frame total-microsecond samples, oldest first — the
    /// sparkline's data. Empty until at least one `frust-perf raw` line has
    /// appeared (i.e. `FRUST_TRACE_RAW` is on for this session).
    pub fn samples(&self) -> impl ExactSizeIterator<Item = u64> + '_ {
        self.ring.iter().copied()
    }

    /// Toggle the panel's per-tab visibility (`t`).
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fixture lines (grammar acceptance) ──────────────────────────────

    #[test]
    fn parses_a_v2_raw_frame_line() {
        let line = "frust-perf raw n=42 total_us=8123 rebuild_us=120 layout_us=340 \
                     paint_us=210 encode_us=980 present_us=430 skipped=0";
        assert_eq!(
            parse_perf_line(line),
            Some(PerfLine::Raw(RawFrame {
                n: 42,
                total_us: 8123,
                rebuild_us: 120,
                layout_us: 340,
                paint_us: 210,
                encode_us: 980,
                present_us: 430,
                skipped: false,
            }))
        );
    }

    #[test]
    fn parses_a_skipped_raw_frame_line() {
        let line = "frust-perf raw n=7 total_us=0 rebuild_us=0 layout_us=0 paint_us=0 \
                     encode_us=0 present_us=0 skipped=1";
        let Some(PerfLine::Raw(frame)) = parse_perf_line(line) else {
            panic!("expected a Raw line");
        };
        assert!(frame.skipped);
    }

    #[test]
    fn parses_a_frame_summary_line() {
        let line = "frust-perf frame n=64 total_p50_ms=8 total_p95_ms=14 total_p99_ms=22 \
                     rebuild_p95_ms=1 layout_p95_ms=2 paint_p95_ms=2 encode_p95_ms=3 \
                     present_p95_ms=2 over_60hz=3 over_120hz=10 skipped=1 total_frames=512";
        assert_eq!(
            parse_perf_line(line),
            Some(PerfLine::Frame(FrameSummary {
                n: 64,
                total_p50_ms: 8,
                total_p95_ms: 14,
                total_p99_ms: 22,
                skipped: 1,
                total_frames: 512,
            })),
            "extra fields the panel doesn't need (rebuild_p95_ms, over_60hz, …) are ignored, \
             not fatal to the parse"
        );
    }

    #[test]
    fn parses_a_startup_line_with_multiple_spans() {
        let line =
            "frust-perf startup app_created=2ms surface_ready=18ms first_frame_presented=45ms";
        assert_eq!(
            parse_perf_line(line),
            Some(PerfLine::Startup(StartupSummary {
                spans: vec![
                    ("app_created".to_string(), 2),
                    ("surface_ready".to_string(), 18),
                    ("first_frame_presented".to_string(), 45),
                ],
            }))
        );
    }

    #[test]
    fn parses_a_bare_startup_line_with_no_spans() {
        assert_eq!(
            parse_perf_line("frust-perf startup"),
            Some(PerfLine::Startup(StartupSummary { spans: vec![] }))
        );
    }

    // ── Tolerant-parsing / non-perf-line cases ──────────────────────────

    #[test]
    fn ordinary_log_lines_are_not_perf_lines() {
        assert_eq!(parse_perf_line("   Compiling huddle v0.1.0"), None);
        assert_eq!(parse_perf_line("app: booting up"), None);
        assert_eq!(parse_perf_line(""), None);
    }

    #[test]
    fn a_raw_line_missing_a_required_field_is_skipped_not_panicking() {
        // No `skipped=` field.
        let line = "frust-perf raw n=1 total_us=1 rebuild_us=1 layout_us=1 paint_us=1 \
                     encode_us=1 present_us=1";
        assert_eq!(parse_perf_line(line), None);
    }

    #[test]
    fn a_raw_line_with_a_non_numeric_field_is_skipped_not_panicking() {
        let line = "frust-perf raw n=oops total_us=1 rebuild_us=1 layout_us=1 paint_us=1 \
                     encode_us=1 present_us=1 skipped=0";
        assert_eq!(parse_perf_line(line), None);
    }

    #[test]
    fn a_frame_summary_line_missing_a_field_is_skipped_not_panicking() {
        let line = "frust-perf frame n=1 total_p50_ms=1 total_p95_ms=1";
        assert_eq!(parse_perf_line(line), None);
    }

    #[test]
    fn a_startup_span_with_a_malformed_token_drops_only_that_token() {
        let line = "frust-perf startup good=5ms garbage first=notanumberms";
        assert_eq!(
            parse_perf_line(line),
            Some(PerfLine::Startup(StartupSummary {
                spans: vec![("good".to_string(), 5)],
            }))
        );
    }

    #[test]
    fn an_unrecognized_frust_perf_kind_is_not_a_perf_line() {
        assert_eq!(parse_perf_line("frust-perf plugin op=write"), None);
    }

    #[test]
    fn a_line_still_carrying_ansi_codes_does_not_match() {
        // Callers must strip ANSI first — a raw line whose prefix is itself
        // colorized never matches the plain `frust-perf ` prefix.
        let colored = "\u{1b}[2mfrust-perf startup app_created=1ms\u{1b}[0m";
        assert_eq!(parse_perf_line(colored), None);
    }

    // ── PerfPanel ────────────────────────────────────────────────────────

    #[test]
    fn panel_starts_with_no_data_and_hidden() {
        let panel = PerfPanel::default();
        assert!(!panel.has_data());
        assert!(!panel.visible);
        assert_eq!(panel.samples().len(), 0);
    }

    #[test]
    fn ingest_ignores_non_perf_lines() {
        let mut panel = PerfPanel::default();
        panel.ingest("Compiling huddle v0.1.0");
        assert!(!panel.has_data());
    }

    #[test]
    fn ingest_a_raw_line_feeds_the_sample_ring_and_flags_data() {
        let mut panel = PerfPanel::default();
        panel.ingest(
            "frust-perf raw n=1 total_us=100 rebuild_us=1 layout_us=1 paint_us=1 \
             encode_us=1 present_us=1 skipped=0",
        );
        assert!(panel.has_data());
        assert_eq!(panel.samples().collect::<Vec<_>>(), vec![100]);
    }

    #[test]
    fn ingest_a_frame_summary_sets_last_frame_without_touching_the_ring() {
        let mut panel = PerfPanel::default();
        panel.ingest(
            "frust-perf frame n=1 total_p50_ms=8 total_p95_ms=14 total_p99_ms=22 skipped=0 \
             total_frames=1",
        );
        assert!(panel.has_data());
        assert_eq!(panel.samples().len(), 0);
        assert_eq!(panel.last_frame.as_ref().map(|f| f.total_p50_ms), Some(8));
    }

    #[test]
    fn ingest_a_startup_line_sets_last_startup() {
        let mut panel = PerfPanel::default();
        panel.ingest("frust-perf startup first_frame_presented=45ms");
        assert!(panel.has_data());
        assert_eq!(panel.last_startup.as_ref().map(|s| s.spans.len()), Some(1));
    }

    #[test]
    fn ring_evicts_oldest_past_frame_ring_cap() {
        let mut panel = PerfPanel::default();
        for i in 0..(FRAME_RING_CAP as u64 + 5) {
            panel.ingest(&format!(
                "frust-perf raw n={i} total_us={i} rebuild_us=0 layout_us=0 paint_us=0 \
                 encode_us=0 present_us=0 skipped=0"
            ));
        }
        let samples: Vec<u64> = panel.samples().collect();
        assert_eq!(samples.len(), FRAME_RING_CAP);
        // The oldest 5 (total_us 0..5) were evicted; the ring starts at 5.
        assert_eq!(samples[0], 5);
        assert_eq!(*samples.last().unwrap(), FRAME_RING_CAP as u64 + 4);
    }

    #[test]
    fn toggle_flips_visibility() {
        let mut panel = PerfPanel::default();
        assert!(!panel.visible);
        panel.toggle();
        assert!(panel.visible);
        panel.toggle();
        assert!(!panel.visible);
    }
}
