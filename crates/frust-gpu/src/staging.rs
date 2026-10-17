//! [`StagingRing`]: a handful of persistently reused, CPU-mapped staging
//! buffers a renderer writes one frame's uploads into, then copies out of
//! inside the frame's own command encoder.
//!
//! # Why a ring, when `Queue::write_buffer` already stages
//!
//! `wgpu`'s `Queue::write_buffer`/`write_texture` (and `write_buffer_with`)
//! are correct by construction — the bytes are copied into queue-owned staging
//! memory at call time and the device-side copy is ordered ahead of the next
//! submission — but on native backends every call creates a *new* staging
//! buffer: a driver buffer object, a memory sub-allocation and a mapping, all
//! torn down again once the submission retires. A frame issuing half a dozen
//! such writes pays that object churn half a dozen times per frame, every
//! frame, for data whose size barely changes.
//!
//! The ring keeps the staging memory instead. Each slot is one
//! `MAP_WRITE | COPY_SRC` buffer; a frame writes every upload into one slot
//! back to back, records `copy_buffer_to_buffer`/`copy_buffer_to_texture` from
//! it into the frame's own encoder, and hands the slot back to the GPU. The
//! steady state creates no buffer at all.
//!
//! # The fence: re-mapped on submit
//!
//! A slot is written by the CPU only while it is mapped, and a mapped buffer
//! cannot be used by a submission, so the slot is unmapped when the frame
//! finishes ([`StagingFrame::finish`]) and re-mapped once the GPU is done
//! reading it. The re-map is attached to the *caller's* encoder with
//! `CommandEncoder::map_buffer_on_submit`: whoever submits that encoder
//! starts the mapping as part of the submit, and `wgpu` completes it only after
//! that submission's work has finished. The completion callback is the slot's
//! fence — it flips the slot back to [`SlotState::Ready`], and a slot in any
//! other state is never handed out. That keeps the "a renderer never submits
//! the caller's encoder" contract intact: the ring needs no hand-back from the
//! submit site, because the encoder itself carries the fence there.
//!
//! The callback runs inside whatever call next processes completed work —
//! `Queue::submit` does so after every submission, as does `Device::poll`.
//! [`StagingRing::begin`] polls once (without waiting) when every slot is
//! still in flight, so a frame that catches the GPU just finishing does not
//! needlessly give up its slot.
//!
//! An encoder that is dropped rather than submitted drops the mapping request
//! with it, and the callback with it: the slot is then [`SlotState::Abandoned`]
//! — unmapped, read by nothing — and is replaced by a fresh mapped buffer the
//! next time it is picked. A failed mapping (a lost device) is
//! [`SlotState::Failed`] and handled the same way.
//!
//! # Slots and growth
//!
//! [`STAGING_SLOTS`] slots: the swapchain lets two frames be in flight
//! (`DESIRED_MAXIMUM_FRAME_LATENCY` in [`crate::surface`]), and the frame being
//! recorded needs a third. Slots are created on first use, each at the next
//! power of two of what the frame needs (never below [`MIN_SLOT_BYTES`]); a
//! frame that outgrows a free slot replaces that slot's buffer with a larger
//! one. Replacing a slot never invalidates work in flight: only a
//! [`SlotState::Ready`] slot is replaced, and `wgpu` keeps a dropped buffer
//! alive until every submission naming it has retired.
//!
//! A frame larger than the ring's per-slot ceiling, or one that finds every
//! slot still in flight, gets no slot ([`StagingRing::begin`] answers `None`)
//! and the caller falls back to queue writes for that frame — the same bytes
//! by the other route, so a refusal costs time, never pixels.
//!
//! # Host-testable split
//!
//! The slot policy — which slot a frame takes, when one is created or grown,
//! when one is refused, how offsets are aligned — lives in the buffer-generic
//! [`StagingRing`] and [`SlotLease`] and is exercised by host tests with a
//! fake [`StagingAllocator`] and a fake fence ([`SlotFence`] signalled by
//! hand). Only [`StagingRing::begin`], [`StagingFrame`] and the `wgpu::Device`
//! allocator touch a live device.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// How many slots a ring keeps: the swapchain's two frames in flight plus the
/// one being recorded.
pub const STAGING_SLOTS: usize = 3;

/// The smallest buffer a slot is ever created at, so a ring whose frames grow
/// a little at a time does not reallocate on every one of them.
pub const MIN_SLOT_BYTES: u64 = 64 * 1024;

/// The largest buffer a slot is grown to. A frame needing more than this gets
/// no slot and falls back to queue writes, rather than every slot of the ring
/// being held at a size one outlier frame asked for.
pub const MAX_SLOT_BYTES: u64 = 32 * 1024 * 1024;

/// The usages every slot buffer is created with: written through a mapping,
/// read by copy commands.
pub const STAGING_USAGE: wgpu::BufferUsages =
    wgpu::BufferUsages::MAP_WRITE.union(wgpu::BufferUsages::COPY_SRC);

/// The alignment a buffer-to-buffer copy's source offset is placed at.
///
/// `wgpu` needs [`wgpu::COPY_BUFFER_ALIGNMENT`] (4); sixteen keeps every
/// record a uniform or vertex stream holds on its natural boundary as well.
pub const BUFFER_COPY_ALIGNMENT: u64 = 16;

/// The alignment a buffer-to-texture copy's source offset is placed at.
///
/// `wgpu` needs a multiple of the texel block size; 512 also satisfies the
/// strictest native placement rule (D3D12's texture data placement
/// alignment), so no backend has to split the copy.
pub const TEXTURE_COPY_ALIGNMENT: u64 = 512;

/// `value` rounded up to a multiple of `alignment` (an `alignment` of zero is
/// treated as one), or `None` when the rounding overflows.
#[must_use]
pub fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value.checked_next_multiple_of(alignment.max(1))
}

/// The capacity a slot is created or grown at to hold `needed` bytes: the
/// next power of two, never below [`MIN_SLOT_BYTES`].
#[must_use]
pub fn slot_capacity(needed: u64) -> u64 {
    needed
        .checked_next_power_of_two()
        .unwrap_or(needed)
        .max(MIN_SLOT_BYTES)
}

/// Where one slot is in its frame cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// Mapped and read by no submission: the only state a slot is handed out
    /// in.
    Ready,
    /// Handed to a frame that is still writing into it.
    Recording,
    /// Unmapped and carried by an encoder: waiting for that encoder's
    /// submission to finish and the re-map to complete.
    InFlight,
    /// Its encoder was dropped without being submitted, so no re-map is
    /// coming: the slot is unmapped and idle, and is replaced when next
    /// picked.
    Abandoned,
    /// Its re-map failed (a lost device): replaced when next picked.
    Failed,
}

impl SlotState {
    const fn to_u8(self) -> u8 {
        match self {
            Self::Ready => 0,
            Self::Recording => 1,
            Self::InFlight => 2,
            Self::Abandoned => 3,
            Self::Failed => 4,
        }
    }

    const fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::Ready,
            1 => Self::Recording,
            2 => Self::InFlight,
            3 => Self::Abandoned,
            _ => Self::Failed,
        }
    }
}

/// One slot's state, shared between the ring and the completion callback that
/// signals it.
///
/// Atomic because that callback runs on whichever thread next processes
/// completed GPU work, which need not be the thread recording frames.
#[derive(Debug, Clone)]
pub struct SlotFence(Arc<AtomicU8>);

impl SlotFence {
    /// A fence for a freshly created, mapped slot.
    #[must_use]
    pub fn ready() -> Self {
        Self(Arc::new(AtomicU8::new(SlotState::Ready.to_u8())))
    }

    /// The slot's current state.
    #[must_use]
    pub fn state(&self) -> SlotState {
        SlotState::from_u8(self.0.load(Ordering::Acquire))
    }

    fn set(&self, state: SlotState) {
        self.0.store(state.to_u8(), Ordering::Release);
    }

    /// Moves a recording slot in flight and returns the token its completion
    /// callback carries.
    #[must_use]
    pub fn arm(&self) -> FenceToken {
        self.set(SlotState::InFlight);
        FenceToken {
            fence: Some(Arc::clone(&self.0)),
        }
    }
}

/// The one-shot signal a slot in flight is waiting for.
///
/// [`Self::signal`] is called by the re-map's completion callback. A token
/// dropped without being signalled — the encoder carrying it was dropped
/// unsubmitted, so its callback was dropped too — marks the slot
/// [`SlotState::Abandoned`] instead of leaving it in flight forever.
#[derive(Debug)]
pub struct FenceToken {
    fence: Option<Arc<AtomicU8>>,
}

impl FenceToken {
    /// The re-map completed (`mapped`) or failed: the slot is
    /// [`SlotState::Ready`] or [`SlotState::Failed`].
    pub fn signal(mut self, mapped: bool) {
        if let Some(fence) = self.fence.take() {
            let state = if mapped {
                SlotState::Ready
            } else {
                SlotState::Failed
            };
            fence.store(state.to_u8(), Ordering::Release);
        }
    }
}

impl Drop for FenceToken {
    fn drop(&mut self) {
        if let Some(fence) = self.fence.take() {
            // Only a slot still in flight is abandoned: one already signalled
            // took the `signal` path above and never reaches here.
            let _ = fence.compare_exchange(
                SlotState::InFlight.to_u8(),
                SlotState::Abandoned.to_u8(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

/// Creates slot buffers.
///
/// Exists so the ring's policy takes a creation *capability* rather than a
/// `wgpu::Device`: a host test implements it with a counter and plain ids.
/// `wgpu::Device` is the only production implementation.
pub trait StagingAllocator {
    /// The created buffer — `wgpu::Buffer` in production.
    type Buffer: Clone;

    /// A [`STAGING_USAGE`] buffer of `size` bytes, already mapped for writing.
    fn create_mapped(&self, size: u64) -> Self::Buffer;
}

impl StagingAllocator for wgpu::Device {
    type Buffer = wgpu::Buffer;

    fn create_mapped(&self, size: u64) -> wgpu::Buffer {
        self.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frust-gpu staging ring slot"),
            size,
            usage: STAGING_USAGE,
            mapped_at_creation: true,
        })
    }
}

/// What a ring has done so far — a diagnostic, never a decision input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StagingStats {
    /// Frames handed a slot.
    pub frames: u64,
    /// Frames refused because every slot was still in flight.
    pub busy: u64,
    /// Frames refused because they needed more than a slot may hold.
    pub too_large: u64,
    /// Slot buffers created, first creations and replacements both.
    pub buffers_created: u64,
}

#[derive(Debug)]
struct Slot<B> {
    buffer: B,
    capacity: u64,
    fence: SlotFence,
}

/// The slots, their capacities and fences: one frame's staging memory at a
/// time, reused once the GPU is done with it.
///
/// Generic over the buffer type for the reason the module header gives; the
/// engine uses the default `StagingRing<wgpu::Buffer>`.
#[derive(Debug)]
pub struct StagingRing<B = wgpu::Buffer> {
    slots: Vec<Slot<B>>,
    max_slots: usize,
    max_slot_bytes: u64,
    stats: StagingStats,
}

impl<B> Default for StagingRing<B> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B> StagingRing<B> {
    /// An empty ring of [`STAGING_SLOTS`] slots of at most
    /// [`MAX_SLOT_BYTES`] each. No buffer exists until the first frame.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(STAGING_SLOTS, MAX_SLOT_BYTES)
    }

    /// An empty ring with its slot count and per-slot ceiling stated.
    #[must_use]
    pub fn with_limits(max_slots: usize, max_slot_bytes: u64) -> Self {
        Self {
            slots: Vec::with_capacity(max_slots),
            max_slots: max_slots.max(1),
            max_slot_bytes,
            stats: StagingStats::default(),
        }
    }

    /// What this ring has done so far.
    #[must_use]
    pub fn stats(&self) -> StagingStats {
        self.stats
    }

    /// How many slots exist.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Slot `index`'s capacity and state, if it exists.
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<(u64, SlotState)> {
        self.slots
            .get(index)
            .map(|slot| (slot.capacity, slot.fence.state()))
    }

    /// Whether some slot could be handed out right now without waiting on the
    /// GPU: a ready slot, a replaceable one, or room for a new one.
    fn has_free_slot(&self) -> bool {
        self.slots.len() < self.max_slots
            || self.slots.iter().any(|slot| {
                matches!(
                    slot.fence.state(),
                    SlotState::Ready | SlotState::Abandoned | SlotState::Failed
                )
            })
    }
}

impl<B: Clone> StagingRing<B> {
    /// Hands out a slot holding at least `bytes`, or `None` when the frame
    /// needs more than a slot may hold or every slot is still in flight.
    ///
    /// In order of preference: a ready slot already large enough (no
    /// allocation), a ready slot too small or an abandoned/failed one
    /// replaced at [`slot_capacity`]`(bytes)`, a new slot while the ring has
    /// fewer than its limit. A slot in flight is never handed out.
    pub fn acquire<A>(&mut self, allocator: &A, bytes: u64) -> Option<SlotLease<B>>
    where
        A: StagingAllocator<Buffer = B>,
    {
        let bytes = bytes.max(wgpu::COPY_BUFFER_ALIGNMENT);
        if bytes > self.max_slot_bytes {
            self.stats.too_large = self.stats.too_large.saturating_add(1);
            return None;
        }
        let capacity = slot_capacity(bytes).min(self.max_slot_bytes.max(bytes));

        let fitting = self
            .slots
            .iter()
            .position(|slot| slot.fence.state() == SlotState::Ready && slot.capacity >= bytes);
        let index = if let Some(index) = fitting {
            index
        } else if let Some(index) = self.slots.iter().position(|slot| {
            matches!(
                slot.fence.state(),
                SlotState::Ready | SlotState::Abandoned | SlotState::Failed
            )
        }) {
            let slot = self.slots.get_mut(index)?;
            slot.buffer = allocator.create_mapped(capacity);
            slot.capacity = capacity;
            slot.fence = SlotFence::ready();
            self.stats.buffers_created = self.stats.buffers_created.saturating_add(1);
            index
        } else if self.slots.len() < self.max_slots {
            self.slots.push(Slot {
                buffer: allocator.create_mapped(capacity),
                capacity,
                fence: SlotFence::ready(),
            });
            self.stats.buffers_created = self.stats.buffers_created.saturating_add(1);
            self.slots.len() - 1
        } else {
            self.stats.busy = self.stats.busy.saturating_add(1);
            return None;
        };

        let slot = self.slots.get(index)?;
        slot.fence.set(SlotState::Recording);
        self.stats.frames = self.stats.frames.saturating_add(1);
        Some(SlotLease {
            index,
            buffer: slot.buffer.clone(),
            capacity: slot.capacity,
            cursor: 0,
            fence: slot.fence.clone(),
        })
    }
}

/// One frame's hold on one slot: the bump cursor its uploads are placed at.
///
/// Released by [`Self::finish`], which moves the slot in flight and returns
/// the token its fence waits on. A lease dropped without finishing leaves the
/// slot [`SlotState::Abandoned`] — whatever it recorded is still correct to
/// submit, but no re-map was asked for, so the slot is replaced rather than
/// reused.
#[derive(Debug)]
pub struct SlotLease<B> {
    index: usize,
    buffer: B,
    capacity: u64,
    cursor: u64,
    fence: SlotFence,
}

impl<B> SlotLease<B> {
    /// Which slot of the ring this is.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The slot's buffer, which every copy this frame records reads from.
    #[must_use]
    pub fn buffer(&self) -> &B {
        &self.buffer
    }

    /// The slot's capacity in bytes.
    #[must_use]
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Bytes placed so far, alignment padding included.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.cursor
    }

    /// Places `len` bytes at the next multiple of `alignment`, returning
    /// their offset, or `None` (placing nothing) when they do not fit.
    pub fn reserve(&mut self, len: u64, alignment: u64) -> Option<u64> {
        let offset = align_up(self.cursor, alignment)?;
        let end = offset.checked_add(len)?;
        if end > self.capacity {
            return None;
        }
        self.cursor = end;
        Some(offset)
    }

    /// Ends the frame's hold: the slot goes in flight, waiting on the token.
    #[must_use]
    pub fn finish(mut self) -> FenceToken {
        let token = self.fence.arm();
        // Disarm the drop below: the slot is in flight, not abandoned.
        self.cursor = u64::MAX;
        token
    }
}

impl<B> Drop for SlotLease<B> {
    fn drop(&mut self) {
        if self.cursor != u64::MAX && self.fence.state() == SlotState::Recording {
            self.fence.set(SlotState::Abandoned);
        }
    }
}

impl StagingRing<wgpu::Buffer> {
    /// Opens one frame's staging: a mapped slot of at least `bytes`, or `None`
    /// when the caller should fall back to queue writes this frame (see
    /// [`Self::acquire`]).
    ///
    /// When every slot is still in flight the device is polled once, without
    /// waiting, so a slot whose submission has already finished is re-mapped
    /// rather than skipped.
    pub fn begin(&mut self, device: &wgpu::Device, bytes: u64) -> Option<StagingFrame> {
        if !self.has_free_slot() {
            let _ = device.poll(wgpu::PollType::Poll);
        }
        let lease = self.acquire(device, bytes)?;
        let view = lease
            .buffer()
            .slice(..lease.capacity())
            .get_mapped_range_mut();
        match view {
            Ok(view) => Some(StagingFrame {
                lease: Some(lease),
                view: Some(view),
            }),
            // Unreachable while the fence and the mapping agree; a slot that
            // reads ready but cannot be viewed is dropped as abandoned (the
            // lease's own drop) and the frame takes the queue-write route.
            Err(_) => None,
        }
    }
}

/// One frame's open slot: a mapped view to write uploads into and the copy
/// commands that move them to their destinations.
///
/// Every copy is recorded into the encoder the caller passes, at the point
/// the caller calls — so a renderer records its copies ahead of the passes
/// that read their destinations. [`Self::finish`] must run before that encoder
/// is submitted: it unmaps the slot (a mapped buffer cannot be submitted) and
/// attaches the slot's fence to the encoder. A frame dropped without it
/// unmaps the slot and leaves it abandoned, which is still safe to submit.
#[derive(Debug)]
pub struct StagingFrame {
    lease: Option<SlotLease<wgpu::Buffer>>,
    view: Option<wgpu::BufferViewMut>,
}

impl StagingFrame {
    /// Bytes placed so far, alignment padding included.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.lease.as_ref().map_or(0, SlotLease::used)
    }

    /// The slot this frame writes into.
    #[must_use]
    pub fn slot(&self) -> Option<usize> {
        self.lease.as_ref().map(SlotLease::index)
    }

    /// Writes `bytes` into the slot and records their copy to `dst` at
    /// `dst_offset`, answering whether it did; `false` (nothing written,
    /// nothing recorded) means the caller writes them by queue instead.
    ///
    /// Refused when they do not fit the slot or are not a whole number of
    /// [`wgpu::COPY_BUFFER_ALIGNMENT`] units, which a copy command cannot
    /// move.
    pub fn copy_to_buffer(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        bytes: &[u8],
        dst: &wgpu::Buffer,
        dst_offset: u64,
    ) -> bool {
        self.copy_parts_to_buffer(encoder, &[bytes], dst, dst_offset)
    }

    /// [`Self::copy_to_buffer`] over `parts` laid back to back: one copy
    /// command for data the caller holds in pieces, landing in `dst` exactly
    /// as the pieces' concatenation. Only the whole is held to the copy
    /// alignment, not each piece.
    pub fn copy_parts_to_buffer(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        parts: &[&[u8]],
        dst: &wgpu::Buffer,
        dst_offset: u64,
    ) -> bool {
        let len: u64 = parts.iter().map(|part| part.len() as u64).sum();
        if len == 0 {
            return true;
        }
        if !len.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
            || !dst_offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        {
            return false;
        }
        let (Some(lease), Some(view)) = (self.lease.as_mut(), self.view.as_mut()) else {
            return false;
        };
        let Some(offset) = lease.reserve(len, BUFFER_COPY_ALIGNMENT) else {
            return false;
        };
        let Ok(mut start) = usize::try_from(offset) else {
            return false;
        };
        for part in parts {
            let end = start + part.len();
            view.slice(start..end).copy_from_slice(part);
            start = end;
        }
        encoder.copy_buffer_to_buffer(lease.buffer(), offset, dst, dst_offset, len);
        true
    }

    /// Writes one texture upload of `rows` rows of `bytes_per_row` bytes into
    /// the slot — `fill` is handed exactly that many bytes of the mapping and
    /// must write every one of them — and records its copy to `dst` over
    /// `extent`, answering whether it did. `false` (nothing written, nothing
    /// recorded) means the caller writes the texture by queue instead.
    ///
    /// Refused when the upload does not fit the slot, or when `bytes_per_row`
    /// is not a multiple of [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`], which a
    /// buffer-to-texture copy requires (a queue write does not).
    pub fn copy_to_texture(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        dst: wgpu::TexelCopyTextureInfo<'_>,
        bytes_per_row: u32,
        rows: u32,
        extent: wgpu::Extent3d,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) -> bool {
        if !bytes_per_row.is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) {
            return false;
        }
        let len = u64::from(bytes_per_row) * u64::from(rows);
        if len == 0 {
            return false;
        }
        let (Some(lease), Some(view)) = (self.lease.as_mut(), self.view.as_mut()) else {
            return false;
        };
        let Some(offset) = lease.reserve(len, TEXTURE_COPY_ALIGNMENT) else {
            return false;
        };
        let (Ok(start), Ok(end)) = (usize::try_from(offset), usize::try_from(offset + len)) else {
            return false;
        };
        fill(view.slice(start..end));
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: lease.buffer(),
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(rows),
                },
            },
            dst,
            extent,
        );
        true
    }

    /// Ends the frame: unmaps the slot and attaches its fence to `encoder`, so
    /// the slot is re-mapped — and handed out again — only once the
    /// submission carrying `encoder` has finished on the GPU.
    pub fn finish(mut self, encoder: &wgpu::CommandEncoder) {
        drop(self.view.take());
        if let Some(lease) = self.lease.take() {
            lease.buffer().unmap();
            let buffer = lease.buffer().clone();
            let token = lease.finish();
            encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Write, .., move |result| {
                token.signal(result.is_ok());
            });
        }
    }
}

impl Drop for StagingFrame {
    fn drop(&mut self) {
        drop(self.view.take());
        if let Some(lease) = self.lease.take() {
            // Unmapped so an encoder that did record this frame's copies can
            // still be submitted; the lease's own drop marks the slot
            // abandoned.
            lease.buffer().unmap();
        }
    }
}
