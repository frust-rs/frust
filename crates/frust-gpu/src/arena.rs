//! The per-frame bump arena every small GPU upload goes through:
//! [`HostBuffer`], the [`BufferSlice`] it hands back, and the
//! [`BufferUploader`] seam that keeps both host-testable.
//!
//! A frame produces many small pieces of data a shader has to read —
//! per-draw uniforms, a handful of vertices, a transform block. Giving each
//! one its own `wgpu::Buffer` means an allocation, a bind group and a
//! separate upload per draw. The arena replaces that with one buffer: every
//! [`HostBuffer::alloc`] appends to a CPU-side staging vector and returns
//! the `(offset, size)` slice the caller will bind at, one
//! [`HostBuffer::flush`] uploads the whole frame's bytes with a single
//! `queue.write_buffer`, and [`HostBuffer::reset`] rewinds the bump pointer
//! at the start of the next frame.
//!
//! # Alignment
//!
//! Every slice is aligned to at least the adapter's
//! [`TierCaps::min_uniform_buffer_offset_alignment`], so any slice is
//! legal as a uniform binding offset — including a dynamic one. That value
//! is 256 under the GLES-3.0/WebGL2 profile and on the iOS Simulator (which
//! misreports its own alignment; [`crate::context`] forces the device
//! request back up to 256 there), which is the strictest target this
//! workspace has, so an arena built against those caps satisfies every
//! looser adapter as well. A caller may ask for *more* alignment for a
//! specific slice — a vertex stride, say — and never gets less.
//!
//! # Grow-only, with a high-water mark
//!
//! The GPU buffer is created on the first flush that has bytes and then
//! reused. It grows geometrically when a frame outgrows it and never
//! shrinks, so the steady state after a few frames is zero buffer
//! allocations per frame; [`HostBuffer::high_water_mark`] reports the
//! largest frame seen, which is what a host tunes an initial size against.
//!
//! # Why there is no 4-deep ring
//!
//! A hand-rolled Vulkan/Metal arena double- or quadruple-buffers, because
//! the CPU writes into persistently mapped memory the GPU may still be
//! reading from an in-flight frame; the ring is what keeps this frame's
//! writes off last frame's bytes.
//!
//! `wgpu::Queue::write_buffer` has no such hazard to guard. The bytes are
//! copied out of the caller's slice into a queue-owned staging allocation at
//! call time, and the actual device-side copy is recorded ahead of the
//! command buffers submitted after it, in queue order — so a write issued
//! for frame N lands after frame N-1's commands have already run, and
//! `wgpu` tracks and reclaims the staging memory itself once the GPU is done
//! with it. The caller never holds a pointer into memory the GPU is reading,
//! which is the only thing a ring exists to arrange. Growing is safe for the
//! same reason: `wgpu` refcounts a replaced buffer until every submission
//! referencing it has retired.
//!
//! What the caller must still respect is ordering within its own frame:
//! rewind with [`HostBuffer::reset`] at frame start, allocate, flush once
//! before submitting the frame's commands. Rewinding a frame whose commands
//! are already recorded but not yet flushed would overwrite the bytes those
//! commands are going to read.

use crate::caps::TierCaps;
use crate::context::DeviceHandle;

/// The usages every arena buffer is created with: the two binding kinds a
/// frame's transient data is read as, plus the copy destination
/// `write_buffer` needs.
pub const ARENA_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::UNIFORM
    .union(wgpu::BufferUsages::VERTEX)
    .union(wgpu::BufferUsages::COPY_DST);

/// The smallest GPU buffer the arena ever creates. A first frame with one
/// 64-byte uniform in it should not lead to a second allocation on the
/// second frame.
pub const MIN_ARENA_CAPACITY: u64 = 64 * 1024;

/// Default debug label for the arena's GPU buffer.
const DEFAULT_LABEL: &str = "frust-gpu host arena";

/// Where a [`HostBuffer::alloc`] call's bytes ended up: a byte range within
/// the arena's single GPU buffer.
///
/// Not `wgpu::BufferSlice` — this is a plain pair of numbers with no
/// borrow of a buffer, so it can be stored in a display list, sorted, or
/// handed to a bind-group builder long after the allocation that produced
/// it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferSlice {
    /// Byte offset into the arena buffer. Always a multiple of the
    /// effective alignment, so it is legal as a uniform binding offset.
    pub offset: u64,
    /// Length in bytes of the data written, excluding any alignment padding
    /// that precedes it.
    pub size: u64,
}

/// Creates and writes the arena's GPU buffer.
///
/// Exists so [`HostBuffer`] takes an upload *capability* rather than a
/// `wgpu::Device`/`wgpu::Queue` pair: a host test implements it with
/// counters and asserts that a second frame reuses the same buffer and
/// issues exactly one write, with no GPU. [`DeviceHandle`] is the only
/// production implementation.
pub trait BufferUploader {
    /// The created buffer — `wgpu::Buffer` in production.
    type Buffer;

    /// Creates an uninitialized buffer of `size` bytes with `usage`.
    fn allocate_buffer(
        &self,
        label: Option<&str>,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> Self::Buffer;

    /// Copies `bytes` into `buffer` at `offset`.
    fn write_buffer(&self, buffer: &Self::Buffer, offset: u64, bytes: &[u8]);
}

impl BufferUploader for DeviceHandle {
    type Buffer = wgpu::Buffer;

    fn allocate_buffer(
        &self,
        label: Option<&str>,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label,
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
        self.queue.write_buffer(buffer, offset, bytes);
    }
}

/// One growable GPU buffer plus the CPU-side staging vector a frame bump-
/// allocates into.
///
/// Generic over the buffer type (`B`) for the same reason
/// [`crate::texture::Texture`] is generic over its texture: the bump
/// pointer, alignment, growth and flush behavior are exercised on a host
/// with a stand-in value. The engine always uses the default
/// `HostBuffer<wgpu::Buffer>`.
#[derive(Debug)]
pub struct HostBuffer<B = wgpu::Buffer> {
    staging: Vec<u8>,
    buffer: Option<B>,
    capacity: u64,
    high_water_mark: u64,
    min_alignment: u32,
    label: String,
    buffer_allocations: u64,
    flushes: u64,
}

impl<B> HostBuffer<B> {
    /// An empty arena aligned for `caps`' adapter. No GPU buffer exists
    /// until the first [`Self::flush`] that has bytes to upload.
    pub fn new(caps: &TierCaps) -> Self {
        Self::with_label(caps, DEFAULT_LABEL)
    }

    /// [`Self::new`] with a caller-chosen debug label on the GPU buffer,
    /// for a host that runs more than one arena and wants to tell them
    /// apart in a graphics debugger.
    pub fn with_label(caps: &TierCaps, label: &str) -> Self {
        Self {
            staging: Vec::new(),
            buffer: None,
            capacity: 0,
            high_water_mark: 0,
            // An adapter that reported 0 would make every offset alignment
            // a no-op; 1 keeps the arithmetic total and the bytes packed.
            min_alignment: caps.min_uniform_buffer_offset_alignment.max(1),
            label: label.to_string(),
            buffer_allocations: 0,
            flushes: 0,
        }
    }

    /// Appends `bytes` to this frame's data and returns the slice they
    /// occupy.
    ///
    /// The offset is rounded up to `max(align, min_uniform_buffer_offset_alignment)`,
    /// so a caller passes the alignment its *own* use needs (a vertex
    /// stride, `1` for "no opinion") and never has to know the adapter's.
    /// Padding bytes are zero-filled.
    pub fn alloc(&mut self, bytes: &[u8], align: u32) -> BufferSlice {
        let align = align.max(self.min_alignment).max(1) as usize;
        let offset = self.staging.len().next_multiple_of(align);
        self.staging.resize(offset, 0);
        self.staging.extend_from_slice(bytes);
        self.high_water_mark = self.high_water_mark.max(self.staging.len() as u64);
        BufferSlice {
            offset: offset as u64,
            size: bytes.len() as u64,
        }
    }

    /// Rewinds the bump pointer for a new frame, keeping the GPU buffer and
    /// its capacity.
    ///
    /// Every [`BufferSlice`] handed out before this call is stale
    /// afterwards — the next frame's allocations reuse those offsets.
    pub fn reset(&mut self) {
        self.staging.clear();
    }

    /// Uploads this frame's bytes in one `write_buffer`, growing (or
    /// creating) the GPU buffer first if the frame outgrew it, and returns
    /// the buffer the frame's [`BufferSlice`] offsets refer to.
    ///
    /// Answers `None` only when nothing has ever been allocated, so there is
    /// no buffer to name. Call once per frame, after the frame's last
    /// [`Self::alloc`] and before submitting commands that read it.
    pub fn flush<U>(&mut self, uploader: &U) -> Option<&B>
    where
        U: BufferUploader<Buffer = B>,
    {
        // `write_buffer` copies whole 4-byte units, so the tail of an
        // odd-length frame is zero-padded up to that boundary rather than
        // dropped.
        let len = self
            .staging
            .len()
            .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT as usize);
        self.staging.resize(len, 0);
        if len == 0 {
            return self.buffer.as_ref();
        }

        if self.buffer.is_none() || self.capacity < len as u64 {
            let capacity = grown_capacity(self.capacity, len as u64);
            self.buffer = Some(uploader.allocate_buffer(Some(&self.label), capacity, ARENA_USAGE));
            self.capacity = capacity;
            self.buffer_allocations += 1;
        }
        self.flushes += 1;

        let buffer = self
            .buffer
            .as_ref()
            .expect("the arena buffer exists once a frame has bytes");
        uploader.write_buffer(buffer, 0, &self.staging);
        Some(buffer)
    }

    /// The GPU buffer backing the arena, once one exists.
    pub fn buffer(&self) -> Option<&B> {
        self.buffer.as_ref()
    }

    /// Bytes allocated so far this frame, padding included.
    pub fn used(&self) -> u64 {
        self.staging.len() as u64
    }

    /// The GPU buffer's current size in bytes, `0` before the first flush.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// The largest single frame the arena has ever held, in bytes — what a
    /// host would size an initial capacity against.
    pub fn high_water_mark(&self) -> u64 {
        self.high_water_mark
    }

    /// How many times a GPU buffer has been created: one for the first
    /// non-empty frame, plus one per growth. A steady-state frame loop
    /// leaves this constant.
    pub fn buffer_allocations(&self) -> u64 {
        self.buffer_allocations
    }

    /// How many uploads the arena has issued — one per non-empty flush.
    pub fn flush_count(&self) -> u64 {
        self.flushes
    }

    /// The alignment floor every slice is rounded up to, from the adapter's
    /// `min_uniform_buffer_offset_alignment`.
    pub fn min_alignment(&self) -> u32 {
        self.min_alignment
    }
}

/// The capacity a buffer of `current` bytes grows to in order to hold
/// `required`.
///
/// Geometric (next power of two, never below [`MIN_ARENA_CAPACITY`]) so a
/// frame that creeps upward by a few bytes each frame does not reallocate
/// every frame, and never smaller than `current` — the arena is grow-only,
/// and a buffer that shrank would invalidate offsets a previous frame's
/// in-flight commands still read. A `required` beyond the adapter's
/// `max_buffer_size` is the device's error to report, not something the
/// arena can round away.
fn grown_capacity(current: u64, required: u64) -> u64 {
    current.max(
        required
            .checked_next_power_of_two()
            .unwrap_or(required)
            .max(MIN_ARENA_CAPACITY),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::DownlevelProfile;

    /// A [`BufferUploader`] with no GPU behind it: the buffer is an id, and
    /// every create/write is counted so growth and per-frame upload counts
    /// are assertable.
    #[derive(Debug, Default)]
    struct FakeUploader {
        created: std::cell::Cell<u32>,
        writes: std::cell::Cell<u32>,
        last_write_len: std::cell::Cell<usize>,
    }

    impl BufferUploader for FakeUploader {
        type Buffer = u32;

        fn allocate_buffer(&self, _label: Option<&str>, _size: u64, _u: wgpu::BufferUsages) -> u32 {
            self.created.set(self.created.get() + 1);
            self.created.get()
        }

        fn write_buffer(&self, _buffer: &u32, _offset: u64, bytes: &[u8]) {
            self.writes.set(self.writes.get() + 1);
            self.last_write_len.set(bytes.len());
        }
    }

    fn arena() -> HostBuffer<u32> {
        HostBuffer::new(&TierCaps::fake(DownlevelProfile::WebGl2))
    }

    #[test]
    fn alloc_aligns_to_the_adapter_minimum() {
        let mut arena = arena();
        assert_eq!(arena.min_alignment(), 256);
        let first = arena.alloc(&[1u8; 4], 1);
        let second = arena.alloc(&[2u8; 300], 1);
        let third = arena.alloc(&[3u8; 1], 1);
        assert_eq!(first, BufferSlice { offset: 0, size: 4 });
        assert_eq!(
            second,
            BufferSlice {
                offset: 256,
                size: 300
            }
        );
        assert_eq!(
            third,
            BufferSlice {
                offset: 768,
                size: 1
            }
        );
    }

    #[test]
    fn alloc_honors_a_larger_caller_alignment() {
        let mut arena = arena();
        arena.alloc(&[0u8; 1], 1);
        let wide = arena.alloc(&[0u8; 8], 1024);
        assert_eq!(wide.offset, 1024);
    }

    #[test]
    fn reset_rewinds_and_keeps_the_high_water_mark() {
        let mut arena = arena();
        arena.alloc(&[0u8; 512], 1);
        assert_eq!(arena.used(), 512);
        assert_eq!(arena.high_water_mark(), 512);

        arena.reset();
        assert_eq!(arena.used(), 0);
        assert_eq!(arena.high_water_mark(), 512);

        arena.alloc(&[0u8; 16], 1);
        assert_eq!(arena.alloc(&[0u8; 16], 1).offset, 256);
        assert_eq!(arena.high_water_mark(), 512);
    }

    #[test]
    fn flush_with_nothing_allocated_creates_no_buffer() {
        let mut arena = arena();
        let uploader = FakeUploader::default();
        assert_eq!(arena.flush(&uploader), None);
        assert_eq!(uploader.created.get(), 0);
        assert_eq!(uploader.writes.get(), 0);
    }

    #[test]
    fn flush_pads_the_upload_to_the_copy_alignment() {
        let mut arena = arena();
        let uploader = FakeUploader::default();
        arena.alloc(&[7u8; 5], 1);
        arena.flush(&uploader);
        assert_eq!(uploader.last_write_len.get(), 8);
    }

    #[test]
    fn growth_is_geometric_and_never_shrinks() {
        assert_eq!(grown_capacity(0, 1), MIN_ARENA_CAPACITY);
        assert_eq!(grown_capacity(0, MIN_ARENA_CAPACITY), MIN_ARENA_CAPACITY);
        assert_eq!(
            grown_capacity(MIN_ARENA_CAPACITY, MIN_ARENA_CAPACITY + 1),
            2 * MIN_ARENA_CAPACITY
        );
        // A smaller frame leaves the capacity where it is.
        assert_eq!(
            grown_capacity(4 * MIN_ARENA_CAPACITY, 8),
            4 * MIN_ARENA_CAPACITY
        );
    }
}
