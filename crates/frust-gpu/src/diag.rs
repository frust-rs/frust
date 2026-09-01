//! GPU-side frame diagnostics: [`TimestampRing`], real GPU time per pass.
//!
//! A frame's `encode_us`/`submit_us` are CPU wall-clock spans around calls that
//! only *record* and *queue* work — they say how long the CPU spent, never how
//! long the GPU took. This module answers the second question directly, by
//! having the GPU stamp its own clock at each pass boundary.
//!
//! # Pass boundaries only
//!
//! Every timestamp this ring takes is written through
//! `wgpu::RenderPassDescriptor::timestamp_writes`, which needs only
//! `wgpu::Features::TIMESTAMP_QUERY`. `CommandEncoder::write_timestamp` and
//! `RenderPass::write_timestamp` are deliberately never used: they need
//! `TIMESTAMP_QUERY_INSIDE_ENCODERS`/`TIMESTAMP_QUERY_INSIDE_PASSES`, which
//! `wgpu` documents as unavailable on tile-based GPUs — Adreno, and Apple's own
//! Metal families — i.e. exactly the mobile adapters this tier exists for. A
//! probe that only works on desktop would answer the wrong question.
//!
//! # Inert unless the device really has the feature
//!
//! `TIMESTAMP_QUERY` is requested only by a `perf-trace` build, and only when
//! the adapter offers it ([`crate::context`]'s `required_features`). This ring
//! makes no assumption about that: it asks the **device** it is handed what it
//! was created with, and when the answer is no it becomes inert — no query set,
//! no buffers, no per-frame work, and [`TimestampRing::latest`] answering
//! `None` forever. An inert ring is what a caller reports as `gpu_q=0`.
//!
//! # The ring
//!
//! Reading a timestamp back means mapping a buffer, which is only ready some
//! frames after the submit that wrote it. So the ring holds [`RING_FRAMES`]
//! independent slots — query set, resolve buffer, readback buffer — and a
//! frame's slot is not read until the ring comes back around to it, by which
//! time its map has long since completed. Nothing on the frame path ever
//! blocks: the map is polled, never waited on, and a slot whose map has not
//! landed simply goes untimed for that frame rather than stalling it.
//!
//! # A pass that draws nothing may measure nothing
//!
//! Metal samples a render pass's counters at the vertex/fragment *stage*
//! boundaries, so a pass that runs neither stage — a pure clear, an empty
//! pass — can leave its query pair unwritten, and the ring drops the pair
//! rather than reporting a garbage span (see [`span_duration`]). This is a
//! measurement gap, never a correctness one: an untimed pass draws exactly
//! what it always did. A span made of several passes, which is the shape every
//! caller here uses, absorbs it — the drawing passes still report.
//!
//! One frame's shape, in call order:
//!
//! 1. [`TimestampRing::begin_frame`] — advance to the next slot and harvest
//!    whatever the frame that last used it left mapped.
//! 2. [`TimestampRing::pass_writes`] once per timed pass, naming the span the
//!    pass belongs to. Each call takes a *fresh* query pair, so several passes
//!    may share one span and their durations add up.
//! 3. [`TimestampRing::resolve`] into the same encoder the passes were recorded
//!    into, before the caller submits it.
//! 4. [`TimestampRing::end_frame`] after that submit (or
//!    [`TimestampRing::abandon_frame`] when the frame was refused and nothing
//!    was submitted).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

/// How many frames-in-flight the ring keeps independent slots for.
///
/// Four is the depth a triple-buffered swapchain plus one in-flight submit can
/// reach, so a slot's map has always completed by the time the ring returns to
/// it — nothing on the frame path ever waits on a map.
pub const RING_FRAMES: usize = 4;

/// The largest number of named spans one frame can be split into.
///
/// A fixed ceiling rather than a `Vec` so [`GpuFrameSpans`] stays `Copy` and a
/// caller can carry one through a frame record without allocating.
pub const MAX_SPANS: usize = 8;

/// How many individual passes one frame can time by default.
///
/// Each timed pass costs its own query pair, so this is what fixes the query
/// set's `count` at `2 * PASSES`. A frame with more passes than this times the
/// first `DEFAULT_PASS_CAPACITY` and leaves the rest untimed — draw-correct
/// either way, since a pass with no `timestamp_writes` is an ordinary pass.
pub const DEFAULT_PASS_CAPACITY: usize = 24;

/// The hard ceiling on a slot's query count, from `wgpu`'s own
/// `QUERY_SET_MAX_QUERIES`. Halved because every pass takes a *pair*.
const MAX_PASS_CAPACITY: usize = (wgpu::QUERY_SET_MAX_QUERIES / 2) as usize;

/// Bytes one resolved timestamp query occupies (`wgpu::QUERY_SIZE`).
const QUERY_BYTES: u64 = wgpu::QUERY_SIZE as u64;

/// A single pass span longer than this is discarded rather than reported.
///
/// A GPU pass measured in whole seconds is not a slow frame, it is a garbage
/// tick pair: an unwritten query, a driver that reset its clock across a
/// submit, or a counter wrap. Reporting it would poison every percentile
/// computed off the raw series, so the span is dropped and its frame simply
/// reads a little low.
const IMPLAUSIBLE_SPAN_NANOS: u64 = 1_000_000_000;

/// The `AtomicU8` states a slot's map callback moves through.
mod map_state {
    /// The map was requested and has not answered yet.
    pub const PENDING: u8 = 0;
    /// The map completed and the buffer holds this frame's resolved queries.
    pub const READY: u8 = 1;
    /// The map failed; the slot is recycled without producing a reading.
    pub const FAILED: u8 = 2;
}

/// One frame's GPU span durations, in the caller's own span order.
///
/// The span *names* are the caller's business — this crate knows only how many
/// there are and which index each pass was charged to. `frust-engine`'s
/// `diag::EngineSpan` is the naming this repo's engine tier uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuFrameSpans {
    durations: [Duration; MAX_SPANS],
    len: usize,
}

impl Default for GpuFrameSpans {
    fn default() -> Self {
        Self {
            durations: [Duration::ZERO; MAX_SPANS],
            len: 0,
        }
    }
}

impl GpuFrameSpans {
    /// An all-zero reading over `len` spans (clamped to [`MAX_SPANS`]).
    #[must_use]
    pub fn zeroed(len: usize) -> Self {
        Self {
            durations: [Duration::ZERO; MAX_SPANS],
            len: len.min(MAX_SPANS),
        }
    }

    /// How many spans this reading covers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether this reading covers no spans at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The duration charged to `index`, or [`Duration::ZERO`] past the end —
    /// a span no pass was charged to is genuinely zero, so an out-of-range
    /// index answering the same is the honest reading rather than a silent
    /// failure mode.
    #[must_use]
    pub fn span(&self, index: usize) -> Duration {
        self.durations.get(index).copied().unwrap_or(Duration::ZERO)
    }

    /// Every span in order.
    #[must_use]
    pub fn as_slice(&self) -> &[Duration] {
        &self.durations[..self.len]
    }

    /// The sum of every span — the frame's total *attributed* GPU pass time.
    ///
    /// Deliberately a sum rather than last-end-minus-first-begin: the gaps
    /// between a frame's passes are queue and driver time this ring cannot
    /// attribute to any pass, and folding them into a "total" would report
    /// work the breakdown below it does not account for.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.as_slice()
            .iter()
            .copied()
            .fold(Duration::ZERO, |acc, span| acc.saturating_add(span))
    }

    /// Charges `span` with `duration` on top of whatever it already holds.
    fn add(&mut self, span: usize, duration: Duration) {
        if let Some(slot) = self.durations.get_mut(span) {
            *slot = slot.saturating_add(duration);
        }
    }
}

/// One frame-in-flight's own query set and its two staging buffers.
#[derive(Debug)]
struct RingFrame {
    queries: wgpu::QuerySet,
    /// `QUERY_RESOLVE | COPY_SRC` — what `resolve_query_set` writes into. It
    /// cannot also be `MAP_READ`, which is why the readback below exists.
    resolve: wgpu::Buffer,
    /// `COPY_DST | MAP_READ` — the mappable copy the harvest reads.
    readback: wgpu::Buffer,
    /// Next free pass pair, bumped by [`TimestampRing::pass_writes`].
    ///
    /// Atomic rather than a `Cell` so the ring stays `Sync`: a renderer that
    /// records its passes off the thread that owns the ring is a shape this
    /// type should not rule out, and the counter is uncontended in every
    /// current caller anyway.
    cursor: AtomicU32,
    /// Which span each pass pair was charged to, indexed by pair.
    owners: Vec<AtomicU8>,
    state: SlotState,
}

/// Where one ring slot is in the record → resolve → map → harvest cycle.
#[derive(Debug)]
enum SlotState {
    /// Holds nothing; free to record into.
    Idle,
    /// This frame's passes are being recorded into it.
    Recording,
    /// Submitted and mapping; `used` pass pairs are staged in `readback`.
    Mapping { used: usize, status: Arc<AtomicU8> },
}

/// A ring of per-frame GPU timestamp query sets — see the module header.
#[derive(Debug)]
pub struct TimestampRing {
    /// How many named spans a frame is split into.
    spans: usize,
    /// How many passes one frame can time.
    passes: usize,
    /// Nanoseconds per timestamp tick, from `queue.get_timestamp_period()`.
    period_ns: f32,
    /// Empty when inert — the one thing every method keys off.
    frames: Vec<RingFrame>,
    /// Which slot the frame in progress is recording into.
    current: usize,
    /// Whether the current frame's slot was actually free to record into.
    armed: bool,
    /// The most recently harvested reading.
    latest: Option<GpuFrameSpans>,
    /// How many frames have been harvested, so a consumer can tell a fresh
    /// reading from the one it already reported.
    harvested: u64,
}

impl TimestampRing {
    /// A ring timing `spans` named spans per frame, over
    /// [`DEFAULT_PASS_CAPACITY`] passes.
    ///
    /// Inert — no GPU resource created at all — when `device` was not created
    /// with `wgpu::Features::TIMESTAMP_QUERY`, which is every build that did
    /// not compile `perf-trace` in and every adapter that does not offer the
    /// feature.
    #[must_use]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, spans: usize, label: &str) -> Self {
        Self::with_capacity(device, queue, spans, DEFAULT_PASS_CAPACITY, label)
    }

    /// [`Self::new`] with an explicit per-frame pass capacity.
    #[must_use]
    pub fn with_capacity(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        spans: usize,
        passes: usize,
        label: &str,
    ) -> Self {
        let spans = spans.min(MAX_SPANS);
        let passes = passes.min(MAX_PASS_CAPACITY);
        if spans == 0 || passes == 0 || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            return Self::inert(spans);
        }

        // A non-finite or non-positive period would scale every tick delta
        // into nonsense, and there is no sound reading to fall back on — an
        // inert ring reporting nothing beats a ring reporting garbage.
        let period_ns = queue.get_timestamp_period();
        if !period_ns.is_finite() || period_ns <= 0.0 {
            log::warn!(
                "frust-gpu: timestamp period {period_ns} is unusable; GPU pass timing is off"
            );
            return Self::inert(spans);
        }

        let query_count = (passes * 2) as u32;
        let staged_bytes = u64::from(query_count) * QUERY_BYTES;
        // `resolve_query_set`'s destination offset is 256-byte aligned; the
        // buffer is rounded up to the same grid so a future non-zero offset
        // needs no re-sizing rule of its own.
        let buffer_bytes = staged_bytes.next_multiple_of(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT);

        let frames = (0..RING_FRAMES)
            .map(|slot| RingFrame {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some(&format!("{label} queries {slot}")),
                    ty: wgpu::QueryType::Timestamp,
                    count: query_count,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{label} resolve {slot}")),
                    size: buffer_bytes,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{label} readback {slot}")),
                    size: buffer_bytes,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                cursor: AtomicU32::new(0),
                owners: (0..passes).map(|_| AtomicU8::new(0)).collect(),
                state: SlotState::Idle,
            })
            .collect();

        Self {
            spans,
            passes,
            period_ns,
            frames,
            // The first `begin_frame` advances onto slot 0.
            current: RING_FRAMES - 1,
            armed: false,
            latest: None,
            harvested: 0,
        }
    }

    /// A ring that measures nothing, for a device without `TIMESTAMP_QUERY`
    /// and for a caller that wants the `gpu_q=0` shape without a GPU at all.
    #[must_use]
    pub fn inert(spans: usize) -> Self {
        Self {
            spans: spans.min(MAX_SPANS),
            passes: 0,
            period_ns: 0.0,
            frames: Vec::new(),
            current: 0,
            armed: false,
            latest: None,
            harvested: 0,
        }
    }

    /// Whether this ring holds real query sets, i.e. whether a frame line
    /// should report `gpu_q=1`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.frames.is_empty()
    }

    /// How many named spans a frame is split into.
    #[must_use]
    pub fn spans(&self) -> usize {
        self.spans
    }

    /// Nanoseconds per timestamp tick, `0.0` on an inert ring.
    #[must_use]
    pub fn timestamp_period_ns(&self) -> f32 {
        self.period_ns
    }

    /// The most recently harvested frame's spans, or `None` before the first
    /// reading lands (and forever on an inert ring).
    ///
    /// The reading lags the CPU frame that asks for it by up to
    /// [`RING_FRAMES`] frames — the price of never blocking on a map. In
    /// steady state one fresh reading lands per frame, so the lag is a
    /// constant offset rather than a gap in the series; [`Self::harvested`]
    /// is how a consumer tells a repeat from a fresh reading.
    #[must_use]
    pub fn latest(&self) -> Option<GpuFrameSpans> {
        self.latest
    }

    /// How many frame readings this ring has harvested.
    #[must_use]
    pub fn harvested(&self) -> u64 {
        self.harvested
    }

    /// Advances onto the next slot and harvests whatever the frame that last
    /// used it left mapped.
    ///
    /// Polls `device` once, without blocking: a map that has not landed leaves
    /// its slot alone and the frame goes untimed rather than stalling on it.
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        if self.frames.is_empty() {
            return;
        }
        self.current = (self.current + 1) % self.frames.len();

        // One non-blocking poll, which is what actually invokes any map
        // callback whose copy has completed. A poll failure is not worth
        // reporting per frame: the slot simply stays pending and this frame
        // goes untimed.
        let _ = device.poll(wgpu::PollType::Poll);

        let spans = self.spans;
        let period_ns = self.period_ns;
        let Some(slot) = self.frames.get_mut(self.current) else {
            self.armed = false;
            return;
        };

        if let SlotState::Mapping { used, status } = &slot.state {
            match status.load(Ordering::Acquire) {
                map_state::PENDING => {
                    // Still in flight: leave the slot exactly as it is (its
                    // readback buffer is mapped-in-progress and must not be
                    // written again) and skip timing this frame.
                    self.armed = false;
                    return;
                }
                map_state::READY => {
                    let reading = harvest(slot, *used, spans, period_ns);
                    slot.readback.unmap();
                    self.latest = Some(reading);
                    self.harvested = self.harvested.saturating_add(1);
                }
                // A failed map leaves `wgpu`'s own map context marked, so the
                // slot is unmapped anyway before it is recorded into again.
                _ => slot.readback.unmap(),
            }
        }

        slot.cursor.store(0, Ordering::Relaxed);
        slot.state = SlotState::Recording;
        self.armed = true;
    }

    /// The `timestamp_writes` for one pass belonging to `span`.
    ///
    /// Each call takes a fresh query pair, so a span made of several passes
    /// (the frame's own strip passes, a layer's page rounds) sums correctly.
    /// `None` — an untimed pass — when the ring is inert, the slot was not
    /// free this frame, `span` is out of range, or the frame has already used
    /// its pass capacity.
    #[must_use]
    pub fn pass_writes(&self, span: usize) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if !self.armed || span >= self.spans {
            return None;
        }
        let slot = self.frames.get(self.current)?;
        let pair = slot.cursor.fetch_add(1, Ordering::Relaxed) as usize;
        if pair >= self.passes {
            return None;
        }
        slot.owners.get(pair)?.store(span as u8, Ordering::Relaxed);
        let first = (pair * 2) as u32;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &slot.queries,
            beginning_of_pass_write_index: Some(first),
            end_of_pass_write_index: Some(first + 1),
        })
    }

    /// Records this frame's query resolve and its copy into the mappable
    /// readback buffer, into the same encoder the timed passes were recorded
    /// into. The caller submits that encoder.
    ///
    /// Only the pairs actually taken are resolved: an untouched query resolves
    /// to whatever the driver left there, and reading it back would be
    /// inventing a span rather than measuring one.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.armed {
            return;
        }
        let Some(slot) = self.frames.get(self.current) else {
            return;
        };
        let used = self.used_pairs(slot);
        if used == 0 {
            return;
        }
        let queries = (used * 2) as u32;
        encoder.resolve_query_set(&slot.queries, 0..queries, &slot.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &slot.resolve,
            0,
            &slot.readback,
            0,
            u64::from(queries) * QUERY_BYTES,
        );
    }

    /// Requests the map that a later [`Self::begin_frame`] harvests. Call
    /// after submitting the encoder [`Self::resolve`] was recorded into.
    pub fn end_frame(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let Some(slot) = self.frames.get_mut(self.current) else {
            return;
        };
        let used = (slot.cursor.load(Ordering::Relaxed) as usize).min(self.passes);
        if used == 0 {
            slot.state = SlotState::Idle;
            return;
        }
        let status = Arc::new(AtomicU8::new(map_state::PENDING));
        let callback = Arc::clone(&status);
        let bytes = (used * 2) as u64 * QUERY_BYTES;
        slot.readback
            .slice(0..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                let state = if result.is_ok() {
                    map_state::READY
                } else {
                    map_state::FAILED
                };
                callback.store(state, Ordering::Release);
            });
        slot.state = SlotState::Mapping { used, status };
    }

    /// Closes a frame that was refused before its encoder was submitted.
    ///
    /// Nothing was recorded and nothing will be, so the slot goes straight
    /// back to idle without a map — mapping a buffer no copy ever reached
    /// would harvest the previous frame's ticks as if they were this one's.
    pub fn abandon_frame(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        if let Some(slot) = self.frames.get_mut(self.current) {
            slot.cursor.store(0, Ordering::Relaxed);
            slot.state = SlotState::Idle;
        }
    }

    /// How many pass pairs the frame in progress actually took.
    fn used_pairs(&self, slot: &RingFrame) -> usize {
        (slot.cursor.load(Ordering::Relaxed) as usize).min(self.passes)
    }
}

/// Reads `used` resolved query pairs out of `slot`'s mapped readback buffer and
/// charges each one to the span its pass named.
fn harvest(slot: &RingFrame, used: usize, spans: usize, period_ns: f32) -> GpuFrameSpans {
    let mut reading = GpuFrameSpans::zeroed(spans);
    let bytes = (used * 2) as u64 * QUERY_BYTES;
    let mapped = slot.readback.slice(0..bytes).get_mapped_range();
    let stride = (QUERY_BYTES * 2) as usize;
    for pair in 0..used {
        let at = pair * stride;
        let Some(chunk) = mapped.get(at..at + stride) else {
            break;
        };
        let (begin, end) = tick_pair(chunk);
        let Some(duration) = span_duration(begin, end, period_ns) else {
            continue;
        };
        let owner = slot
            .owners
            .get(pair)
            .map_or(usize::MAX, |span| span.load(Ordering::Relaxed) as usize);
        reading.add(owner, duration);
    }
    drop(mapped);
    reading
}

/// The `(begin, end)` tick pair a 16-byte resolved chunk holds, little-endian
/// as `wgpu` resolves it. A short chunk answers `(0, 0)`, which
/// [`span_duration`] then rejects — the one shape that cannot be a real span.
fn tick_pair(chunk: &[u8]) -> (u64, u64) {
    fn tick(bytes: Option<&[u8]>) -> u64 {
        bytes
            .and_then(|slice| <[u8; 8]>::try_from(slice).ok())
            .map_or(0, u64::from_le_bytes)
    }
    (tick(chunk.get(0..8)), tick(chunk.get(8..16)))
}

/// One pass's duration from its tick pair, or `None` when the pair cannot be a
/// real measurement.
///
/// Rejected: a non-increasing pair (an unwritten query, or a counter that
/// wrapped across the pass) and anything past [`IMPLAUSIBLE_SPAN_NANOS`] (see
/// that constant). Both are dropped rather than clamped — a clamped garbage
/// value is still reported as a measurement.
fn span_duration(begin: u64, end: u64, period_ns: f32) -> Option<Duration> {
    if end <= begin || !period_ns.is_finite() || period_ns <= 0.0 {
        return None;
    }
    let nanos = (end - begin) as f64 * f64::from(period_ns);
    if !nanos.is_finite() || nanos >= IMPLAUSIBLE_SPAN_NANOS as f64 {
        return None;
    }
    Some(Duration::from_nanos(nanos as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeroed_reading_reports_its_span_count_and_no_time() {
        let reading = GpuFrameSpans::zeroed(4);
        assert_eq!(reading.len(), 4);
        assert!(!reading.is_empty());
        assert_eq!(reading.as_slice().len(), 4);
        assert_eq!(reading.total(), Duration::ZERO);
        for index in 0..4 {
            assert_eq!(reading.span(index), Duration::ZERO);
        }
    }

    #[test]
    fn span_count_is_clamped_to_the_ceiling() {
        assert_eq!(GpuFrameSpans::zeroed(MAX_SPANS + 9).len(), MAX_SPANS);
    }

    #[test]
    fn an_index_past_the_end_reads_zero_rather_than_panicking() {
        let reading = GpuFrameSpans::zeroed(2);
        assert_eq!(reading.span(7), Duration::ZERO);
        assert_eq!(reading.span(usize::MAX), Duration::ZERO);
    }

    #[test]
    fn several_passes_charged_to_one_span_add_up() {
        let mut reading = GpuFrameSpans::zeroed(3);
        reading.add(1, Duration::from_micros(400));
        reading.add(1, Duration::from_micros(250));
        reading.add(2, Duration::from_micros(90));
        assert_eq!(reading.span(0), Duration::ZERO);
        assert_eq!(reading.span(1), Duration::from_micros(650));
        assert_eq!(reading.span(2), Duration::from_micros(90));
        assert_eq!(reading.total(), Duration::from_micros(740));
    }

    #[test]
    fn a_charge_past_the_span_ceiling_is_dropped_not_wrapped() {
        let mut reading = GpuFrameSpans::zeroed(2);
        reading.add(MAX_SPANS, Duration::from_secs(1));
        assert_eq!(reading.total(), Duration::ZERO);
    }

    #[test]
    fn span_duration_scales_ticks_by_the_period() {
        // 1000 ticks at 1ns/tick, and the same 1000 ticks on an adapter whose
        // tick is 38.4ns (a real Adreno-class period) — the same pair must
        // read as two different durations, which is the whole point of
        // scaling by the queue's own period rather than assuming nanoseconds.
        assert_eq!(span_duration(0, 1_000, 1.0), Some(Duration::from_micros(1)));
        assert_eq!(
            span_duration(500, 1_500, 38.4),
            Some(Duration::from_nanos(38_400))
        );
    }

    #[test]
    fn a_non_increasing_tick_pair_is_no_measurement() {
        assert_eq!(span_duration(0, 0, 1.0), None);
        assert_eq!(span_duration(900, 900, 1.0), None);
        assert_eq!(span_duration(1_000, 999, 1.0), None);
    }

    #[test]
    fn an_implausible_span_is_dropped_rather_than_reported() {
        assert_eq!(span_duration(0, IMPLAUSIBLE_SPAN_NANOS, 1.0), None);
        assert_eq!(span_duration(0, u64::MAX, 1.0), None);
        // Just under the ceiling still reads.
        assert!(span_duration(0, IMPLAUSIBLE_SPAN_NANOS - 1, 1.0).is_some());
    }

    #[test]
    fn an_unusable_period_yields_no_measurement() {
        assert_eq!(span_duration(0, 1_000, 0.0), None);
        assert_eq!(span_duration(0, 1_000, -1.0), None);
        assert_eq!(span_duration(0, 1_000, f32::NAN), None);
        assert_eq!(span_duration(0, 1_000, f32::INFINITY), None);
    }

    #[test]
    fn a_tick_pair_reads_little_endian_and_a_short_chunk_reads_zero() {
        let mut chunk = [0_u8; 16];
        chunk[..8].copy_from_slice(&7_u64.to_le_bytes());
        chunk[8..].copy_from_slice(&19_u64.to_le_bytes());
        assert_eq!(tick_pair(&chunk), (7, 19));
        assert_eq!(tick_pair(&chunk[..12]), (7, 0));
        assert_eq!(tick_pair(&[]), (0, 0));
    }

    #[test]
    fn an_inert_ring_measures_nothing_and_never_reports_a_reading() {
        // The `gpu_q=0` shape a device without TIMESTAMP_QUERY produces,
        // exercised with no GPU in the loop at all.
        let mut ring = TimestampRing::inert(4);
        assert!(!ring.is_active());
        assert_eq!(ring.spans(), 4);
        assert_eq!(ring.timestamp_period_ns(), 0.0);
        assert!(ring.pass_writes(0).is_none());
        assert!(ring.latest().is_none());
        ring.end_frame();
        ring.abandon_frame();
        assert!(ring.latest().is_none());
        assert_eq!(ring.harvested(), 0);
    }

    /// A full-screen triangle drawn by the real-adapter test below, so each
    /// timed pass runs the vertex and fragment stages Metal samples its
    /// counters at — see that test's own comment.
    const RING_TEST_WGSL: &str = "\
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.2, 0.4, 0.8, 1.0);
}
";

    /// Blocks on `future` by polling it to completion — `wgpu`'s native
    /// adapter and device requests resolve without an executor driving them,
    /// and this crate has no async runtime of its own.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    #[ignore = "needs a real GPU adapter offering TIMESTAMP_QUERY; run with \
                `cargo test -p frust-gpu -- --ignored` (pin the adapter on a \
                multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
    fn a_real_adapter_reports_plausible_per_pass_gpu_time() {
        block_on(async {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
            );
            let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
                .await
                .expect("no compatible GPU adapter");
            println!("frust-gpu timestamp-ring adapter: {:?}", adapter.get_info());
            if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
                println!("adapter offers no TIMESTAMP_QUERY — the inert path is the whole story");
                return;
            }
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("frust-gpu timestamp ring test device"),
                    required_features: wgpu::Features::TIMESTAMP_QUERY,
                    required_limits: wgpu::Limits::default(),
                    ..Default::default()
                })
                .await
                .expect("failed to create the device");

            // Two spans, three passes charged across them: two into span 0 and
            // one into span 1, so the reading is per-span rather than per-pass
            // and both spans must come back nonzero.
            //
            // Every pass DRAWS. A draw-less pass is not a shortcut here: Metal
            // samples a render pass's counters at the vertex/fragment stage
            // boundaries (`setStartOfVertexSampleIndex`/
            // `setEndOfFragmentSampleIndex`), so a pass that runs neither stage
            // can leave its query pair unwritten — the case
            // [`span_duration`] drops rather than reports.
            let mut ring = TimestampRing::new(&device, &queue, 2, "frust-gpu ring test");
            assert!(
                ring.is_active(),
                "a TIMESTAMP_QUERY device must arm the ring"
            );
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let target = crate::HeadlessTarget::new(&device, 512, 512, format);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("frust-gpu ring test shader"),
                source: wgpu::ShaderSource::Wgsl(RING_TEST_WGSL.into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("frust-gpu ring test pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

            let mut readings = 0_u64;
            for frame in 0..(RING_FRAMES * 3) {
                ring.begin_frame(&device);
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frust-gpu ring test frame"),
                });
                for (span, instances) in [(0_usize, 64_u32), (0, 64), (1, 64)] {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("frust-gpu ring test pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target.view(),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: ring.pass_writes(span),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&pipeline);
                    pass.draw(0..3, 0..instances);
                }
                ring.resolve(&mut encoder);
                queue.submit([encoder.finish()]);
                ring.end_frame();
                // The frame path never waits on a map; a test may, and does,
                // so the harvest is deterministic rather than timing-dependent.
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                if ring.harvested() > readings {
                    readings = ring.harvested();
                    println!("frame {frame}: {:?}", ring.latest());
                }
            }

            let reading = ring
                .latest()
                .expect("a TIMESTAMP_QUERY device must have produced at least one reading");
            assert_eq!(reading.len(), 2);
            assert!(
                reading.span(0) > Duration::ZERO,
                "span 0 covered two real render passes: {reading:?}"
            );
            assert!(
                reading.span(1) > Duration::ZERO,
                "span 1 covered a real render pass: {reading:?}"
            );
            assert_eq!(reading.total(), reading.span(0) + reading.span(1));
            assert!(
                reading.total() < Duration::from_millis(100),
                "three 256x256 clears cannot plausibly take {:?}",
                reading.total()
            );
        });
    }
}
