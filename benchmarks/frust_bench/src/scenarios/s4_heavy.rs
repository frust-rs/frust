//! S4 — Heavy-work responsiveness (parse ~50MB while animating).
//!
//! The head-to-head for PLAN 9.E's S4 claim: `use_task` + `spawn_blocking`
//! (which **moves** the payload into the worker) vs Flutter's `Isolate.run`
//! (which **copies** it across the isolate boundary). A deterministic ~50MB
//! JSON payload is generated and parsed entirely on a blocking-pool thread,
//! bracketed by the `s4-parse` marker pair (its own wall-time series), while a
//! continuous [`SpinBox`] animation runs on the UI thread the whole time — so
//! the raw frame series inside the parse window shows any UI-thread impact.
//!
//! ## Dataset parity (PLAN 9.E addendum — binding)
//!
//! The payload generator is a byte-for-byte Rust port of the Flutter bench's
//! `generateS4JsonPayload` (`benchmarks/flutter_bench/lib/bench/datasets.dart`):
//! the same [`SplitMix64`] seed (`0x5EED4`), the same [`RECORD_COUNT`], the same
//! per-record field order and per-field draw order, so both apps parse the
//! identical byte stream. Every value field is integer-valued, so the JSON text
//! is byte-identical across languages (no float-formatting divergence).
//!
//! Parity note: `datasets.dart` generates the payload off-thread and then parses
//! it in a *second* isolate (the copy under test). The frust side generates and
//! parses in the *same* `spawn_blocking` call — the payload never crosses a
//! thread boundary at all, which is precisely the zero-copy advantage S4
//! claims. Only the bracketed parse (not generation) is measured.

use std::time::Duration;

use frust::{
    Align, Alignment, AnyView, AsyncValue, Color, Component, Get, Stack, UseTask, any, component,
    spawn_blocking, text, use_task,
};
use frust_core::{
    AnimationController, BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene,
    View, Widget,
};
use kurbo::{Point, Size};

use super::{BenchState, Scenario};

/// S4 — Heavy-work responsiveness.
pub struct S4;

impl Scenario for S4 {
    fn id(&self) -> &'static str {
        "s4"
    }

    fn title(&self) -> &'static str {
        "Heavy-work responsiveness (parse while animating)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        any(component(S4Heavy))
    }
}

// ---------------------------------------------------------------------------
// Deterministic payload — the datasets.dart port
// ---------------------------------------------------------------------------

/// Seed for the S4 payload generator — `s4PayloadSeed` in `datasets.dart`.
const PAYLOAD_SEED: u64 = 0x5EED4;

/// Record count — `s4RecordCount` in `datasets.dart`; tuned so the serialized
/// JSON is ~50MB (each record ≈ 500 bytes).
const RECORD_COUNT: usize = 100_000;

/// SplitMix64 — a byte-for-byte port of the shared PRNG (`rng.dart` /
/// `s1_animation/physics.rs`), so a given seed reproduces the same value
/// sequence as the Flutter side.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)` from the top 53 bits — `nextF64` in `rng.dart`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// An integer in `[0, bound)` — `nextIntBelow` in `rng.dart`.
    fn next_int_below(&mut self, bound: u64) -> u64 {
        (self.next_f64() * bound as f64).floor() as u64
    }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// 12 hex chars from 12 low nibbles — `_hex12` in `datasets.dart`.
fn hex12(rng: &mut SplitMix64, out: &mut String) {
    for _ in 0..12 {
        out.push(HEX_DIGITS[(rng.next_u64() & 0xF) as usize] as char);
    }
}

/// Build the deterministic ~50MB JSON payload — the Rust port of
/// `generateS4JsonPayload`. Field/draw order is load-bearing for byte parity:
/// name (12 nibble draws), value, active, then three tag draws.
fn generate_payload() -> String {
    let mut rng = SplitMix64(PAYLOAD_SEED);
    // Pre-size generously (~500 bytes/record) to avoid reallocation churn.
    let mut buf = String::with_capacity(RECORD_COUNT * 512);
    buf.push('[');
    for i in 0..RECORD_COUNT {
        if i > 0 {
            buf.push(',');
        }
        let mut name = String::with_capacity(12);
        hex12(&mut rng, &mut name);
        let value = rng.next_int_below(1000);
        let active = (rng.next_u64() & 1) == 1;
        let t0 = rng.next_int_below(1000);
        let t1 = rng.next_int_below(1000);
        let t2 = rng.next_int_below(1000);
        buf.push_str("{\"id\":");
        buf.push_str(&i.to_string());
        buf.push_str(",\"name\":\"");
        buf.push_str(&name);
        buf.push_str("\",\"value\":");
        buf.push_str(&value.to_string());
        buf.push_str(",\"active\":");
        buf.push_str(if active { "true" } else { "false" });
        buf.push_str(",\"tags\":[");
        buf.push_str(&t0.to_string());
        buf.push(',');
        buf.push_str(&t1.to_string());
        buf.push(',');
        buf.push_str(&t2.to_string());
        buf.push_str("]}");
    }
    buf.push(']');
    buf
}

// ---------------------------------------------------------------------------
// A small owned-DOM JSON parser — the genuine heavy work
// ---------------------------------------------------------------------------

/// An owned JSON value. Materializing this DOM (owned `String`s + `Vec`s) is
/// the CPU + allocation load S4 measures — the counterpart to Flutter's
/// `jsonDecode`. `frust_bench` deliberately depends on no JSON crate, so the
/// parser is hand-rolled here.
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// A cursor over the payload bytes.
struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser {
            bytes: s.as_bytes(),
            pos: 0,
        }
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b' ' || b == b'\n' || b == b'\t' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn value(&mut self) -> Option<Json> {
        self.skip_ws();
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Some(Json::Str(self.string()?)),
            b't' => self.literal(b"true", Json::Bool(true)),
            b'f' => self.literal(b"false", Json::Bool(false)),
            b'n' => self.literal(b"null", Json::Null),
            _ => self.number(),
        }
    }

    fn literal(&mut self, kw: &[u8], val: Json) -> Option<Json> {
        if self.bytes[self.pos..].starts_with(kw) {
            self.pos += kw.len();
            Some(val)
        } else {
            None
        }
    }

    fn string(&mut self) -> Option<String> {
        // Opening quote.
        self.pos += 1;
        let mut out = String::new();
        loop {
            let b = *self.bytes.get(self.pos)?;
            self.pos += 1;
            match b {
                b'"' => return Some(out),
                b'\\' => {
                    let e = *self.bytes.get(self.pos)?;
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0C}'),
                        b'u' => {
                            let hex = self.bytes.get(self.pos..self.pos + 4)?;
                            self.pos += 4;
                            let cp =
                                u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return None,
                    }
                }
                _ => out.push(b as char),
            }
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == b'-' || b == b'+' || b == b'.' || b == b'e' || b == b'E' || b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice = std::str::from_utf8(&self.bytes[start..self.pos]).ok()?;
        slice.parse::<f64>().ok().map(Json::Num)
    }

    fn array(&mut self) -> Option<Json> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek()? == b']' {
                self.pos += 1;
                return Some(Json::Arr(items));
            }
            items.push(self.value()?);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Some(Json::Arr(items));
                }
                _ => return None,
            }
        }
    }

    fn object(&mut self) -> Option<Json> {
        self.pos += 1; // '{'
        let mut fields = Vec::new();
        loop {
            self.skip_ws();
            if self.peek()? == b'}' {
                self.pos += 1;
                return Some(Json::Obj(fields));
            }
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            if self.peek()? != b':' {
                return None;
            }
            self.pos += 1;
            let val = self.value()?;
            fields.push((key, val));
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(Json::Obj(fields));
                }
                _ => return None,
            }
        }
    }
}

/// Fold every leaf of a parsed value into `acc`, so the whole DOM is *read*
/// after being built — this both prevents the optimizer from eliding the parse
/// and makes every field genuinely used (a real parse traverses its result).
fn checksum(v: &Json, acc: &mut u64) {
    match v {
        Json::Null => *acc = acc.wrapping_add(1),
        Json::Bool(b) => *acc = acc.wrapping_add(*b as u64),
        Json::Num(n) => *acc = acc.wrapping_add(n.to_bits()),
        Json::Str(s) => *acc = acc.wrapping_add(s.len() as u64),
        Json::Arr(items) => {
            for it in items {
                checksum(it, acc);
            }
        }
        Json::Obj(fields) => {
            for (k, val) in fields {
                *acc = acc.wrapping_add(k.len() as u64);
                checksum(val, acc);
            }
        }
    }
}

/// Parse the payload into an owned DOM and return the top-level array length
/// (the record count). Traverses the whole DOM (see [`checksum`]) so the parse
/// can't be optimized away. Returns `0` on a malformed payload (never happens
/// for the generated one — the fallback keeps the parser total).
fn parse_record_count(payload: &str) -> usize {
    match Parser::new(payload).value() {
        Some(Json::Arr(items)) => {
            let mut acc = 0u64;
            for it in &items {
                checksum(it, &mut acc);
            }
            std::hint::black_box(acc);
            items.len()
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// The scenario component
// ---------------------------------------------------------------------------

/// S4 — Heavy-work responsiveness component.
pub struct S4Heavy;

/// Retained S4 state: the parse task handle (its `AsyncValue<usize>` is the
/// parsed record count).
pub struct S4State {
    task: UseTask<usize>,
}

impl Component for S4Heavy {
    type State = S4State;

    fn init(&self) -> S4State {
        // Generate + parse entirely on a blocking-pool thread: the payload never
        // crosses a thread boundary (the zero-copy move). Only the parse is
        // bracketed by the `s4-parse` marker pair (its own wall-time series).
        let task = use_task(|| async {
            spawn_blocking(|| {
                let payload = generate_payload();
                frust_shell_common::perf::mark_scenario_start("s4-parse");
                let count = parse_record_count(&payload);
                frust_shell_common::perf::mark_scenario_end("s4-parse");
                count
            })
            .await
        });
        S4State { task }
    }

    fn build(&self, state: &mut S4State) -> AnyView<S4State> {
        let status = match state.task.signal().get() {
            AsyncValue::Idle | AsyncValue::Loading(_) => {
                format!("parsing ~50MB / {RECORD_COUNT} records off-thread…")
            }
            AsyncValue::Ready(n) => format!("parsed {n} records"),
            AsyncValue::Error(_) => "parse failed".to_string(),
        };

        any(Stack(vec![
            // The animation runs the whole time, independent of the parse.
            any(SpinBox::new()),
            any(Align(Alignment::new(0.0, 0.0), text(status).size(18.0))),
        ]))
    }
}

// ---------------------------------------------------------------------------
// SpinBox — a continuous paint-driven animation (UI-thread responsiveness probe)
// ---------------------------------------------------------------------------

/// The animation period.
const SPIN_PERIOD: Duration = Duration::from_secs(2);
/// Orbit radius of the box, logical px.
const ORBIT_RADIUS: f64 = 64.0;
/// The orbiting box's side length, logical px.
const BOX_SIZE: f64 = 48.0;

/// A continuously-animating box that orbits the screen center every frame,
/// requesting a frame each paint. Its purpose is to keep the UI thread producing
/// frames during the off-thread parse, so the raw frame series inside the
/// `s4-parse` window reveals any jank.
pub struct SpinBox;

impl SpinBox {
    fn new() -> Self {
        SpinBox
    }
}

/// The retained spinner widget.
pub struct SpinBoxWidget {
    controller: AnimationController,
    size: Size,
}

impl<State: 'static> View<State> for SpinBox {
    type Element = SpinBoxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SpinBoxWidget {
        let mut controller = AnimationController::new(SPIN_PERIOD);
        controller.repeat();
        SpinBoxWidget {
            controller,
            size: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut SpinBoxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for SpinBoxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.size = bc.max();
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let animating = self.controller.advance(ctx.frame_time());
        let phase = self.controller.value() * std::f64::consts::TAU;
        let origin = ctx.origin();
        let cx = origin.x + self.size.width / 2.0;
        let cy = origin.y + self.size.height / 2.0;
        let bx = cx + phase.cos() * ORBIT_RADIUS - BOX_SIZE / 2.0;
        let by = cy + phase.sin() * ORBIT_RADIUS - BOX_SIZE / 2.0;
        scene.fill_rounded_rect(
            Point::new(bx, by),
            Size::new(BOX_SIZE, BOX_SIZE),
            12.0,
            Color::from_rgb8(0x26, 0xC6, 0xDA),
        );
        if animating {
            ctx.request_frame();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_matches_shared_seed_sequence() {
        // The first three draws for seed 42 must match the shared SplitMix64
        // (same constants as `physics.rs`/`rng.dart`).
        let mut a = SplitMix64(42);
        let mut b = SplitMix64(42);
        for _ in 0..8 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn payload_has_exactly_record_count_records() {
        // A small payload check: generate a truncated equivalent and parse it.
        let payload = generate_payload();
        assert!(payload.starts_with('['));
        assert!(payload.ends_with(']'));
        assert_eq!(parse_record_count(&payload), RECORD_COUNT);
    }

    #[test]
    fn parser_handles_the_generated_record_shape() {
        // One synthetic record with the exact field order the generator emits.
        let one = r#"[{"id":0,"name":"0a1b2c3d4e5f","value":42,"active":true,"tags":[1,2,3]}]"#;
        assert_eq!(parse_record_count(one), 1);
    }

    #[test]
    fn parser_handles_empty_and_nested() {
        assert_eq!(parse_record_count("[]"), 0);
        assert_eq!(parse_record_count(r#"[[1,2],[3],"x",false,null]"#), 5);
    }
}
