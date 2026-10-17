//! Behavioural tests for [`frust_gpu::staging::StagingRing`].
//!
//! The slot policy runs with no GPU: a counting [`StagingAllocator`] stands in
//! for the device and each slot's fence is signalled by hand, so slot
//! rotation, growth, refusal while a fence is pending, abandonment and offset
//! alignment are plain assertions on any host. The `#[ignore]`d tests at the
//! end put the ring on a real device — bytes copied through a slot land in a
//! buffer and a texture exactly, a slot is reused once its submission
//! retires, a dropped encoder abandons its slot safely — and count the heap
//! allocations a staged frame makes against the same uploads issued as queue
//! writes:
//!
//! ```text
//! cargo test -p frust-gpu --test staging -- --ignored --test-threads=1 --nocapture
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use frust_gpu::RenderContext;
use frust_gpu::staging::{
    BUFFER_COPY_ALIGNMENT, MIN_SLOT_BYTES, STAGING_SLOTS, SlotState, StagingAllocator, StagingRing,
    TEXTURE_COPY_ALIGNMENT, align_up, slot_capacity,
};

/// Counts the heap allocations made on a thread while it has counting
/// switched on, so a frame's allocations are measured without the test
/// harness's own (or another thread's) landing in the figure.
struct CountingAllocator;

static COUNTED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
}

// SAFETY: every call is forwarded unchanged to the system allocator; the
// counter is a side effect on an atomic and never touches the allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTED.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: forwarded with the caller's own layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System.alloc` with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTED.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: forwarded with the caller's own layout.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTED.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: `ptr` came from `System` with `layout`; forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The heap allocations `body` makes on this thread.
fn allocations_in(body: impl FnOnce()) -> u64 {
    let before = COUNTED.load(Ordering::Relaxed);
    COUNTING.with(|counting| counting.set(true));
    body();
    COUNTING.with(|counting| counting.set(false));
    COUNTED.load(Ordering::Relaxed) - before
}

/// Hands out plain ids in place of buffers and counts how many it created,
/// so "did that frame allocate?" is a plain assertion.
#[derive(Debug, Default)]
struct FakeAllocator {
    created: Cell<u32>,
    sizes: std::cell::RefCell<Vec<u64>>,
}

impl StagingAllocator for FakeAllocator {
    type Buffer = u32;

    fn create_mapped(&self, size: u64) -> u32 {
        self.created.set(self.created.get() + 1);
        self.sizes.borrow_mut().push(size);
        self.created.get()
    }
}

#[test]
fn slots_rotate_and_a_frame_is_refused_until_a_fence_passes() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::new();

    // One frame per slot, none of them retired yet: three distinct slots.
    let mut tokens = Vec::new();
    for expected in 0..STAGING_SLOTS {
        let lease = ring.acquire(&allocator, 1_000).expect("a free slot");
        assert_eq!(lease.index(), expected, "each frame takes the next slot");
        tokens.push(lease.finish());
    }
    assert_eq!(allocator.created.get(), STAGING_SLOTS as u32);
    for index in 0..STAGING_SLOTS {
        assert_eq!(
            ring.slot(index).map(|(_, state)| state),
            Some(SlotState::InFlight)
        );
    }

    // Every slot in flight: the next frame is refused rather than handed a
    // slot the GPU may still be reading, and no buffer is created for it.
    assert!(
        ring.acquire(&allocator, 1_000).is_none(),
        "every slot is in flight"
    );
    assert_eq!(ring.stats().busy, 1);
    assert_eq!(allocator.created.get(), STAGING_SLOTS as u32);

    // The fake fence passes for the middle slot only: that one is reused,
    // as-is, and the other two are still refused.
    tokens.remove(1).signal(true);
    let lease = ring.acquire(&allocator, 1_000).expect("the retired slot");
    assert_eq!(
        lease.index(),
        1,
        "only the slot whose fence passed is reused"
    );
    tokens.push(lease.finish());
    assert!(
        ring.acquire(&allocator, 1_000).is_none(),
        "the others are still in flight"
    );
    assert_eq!(
        allocator.created.get(),
        STAGING_SLOTS as u32,
        "reuse allocates nothing"
    );
    assert_eq!(ring.stats().frames, STAGING_SLOTS as u64 + 1);
}

#[test]
fn a_free_slot_too_small_grows_to_the_next_power_of_two() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::with_limits(1, u64::MAX);

    let lease = ring.acquire(&allocator, 1_000).expect("a slot");
    assert_eq!(
        lease.capacity(),
        MIN_SLOT_BYTES,
        "small frames get the floor"
    );
    lease.finish().signal(true);

    let lease = ring.acquire(&allocator, 100_000).expect("a grown slot");
    assert_eq!(lease.capacity(), 131_072);
    assert_eq!(lease.index(), 0, "the one slot is replaced, not added to");
    lease.finish().signal(true);
    assert_eq!(*allocator.sizes.borrow(), vec![MIN_SLOT_BYTES, 131_072]);

    // A smaller frame afterwards keeps the grown slot.
    let lease = ring.acquire(&allocator, 5_000).expect("the grown slot");
    assert_eq!(lease.capacity(), 131_072);
    lease.finish().signal(true);
    assert_eq!(allocator.created.get(), 2);
}

#[test]
fn growth_never_replaces_a_slot_in_flight() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::with_limits(2, u64::MAX);

    let small = ring.acquire(&allocator, 1_000).expect("a slot").finish();
    // Slot 0 is in flight and too small: a large frame gets a new slot.
    let large = ring.acquire(&allocator, 200_000).expect("a second slot");
    assert_eq!(large.index(), 1);
    assert_eq!(ring.slot(0), Some((MIN_SLOT_BYTES, SlotState::InFlight)));
    let large = large.finish();

    // Both in flight: a third frame waits, whatever its size.
    assert!(ring.acquire(&allocator, 10).is_none());
    small.signal(true);
    large.signal(true);
    let lease = ring
        .acquire(&allocator, 150_000)
        .expect("the large slot fits");
    assert_eq!(
        lease.index(),
        1,
        "a slot that already fits wins over growing one"
    );
    drop(lease.finish());
    assert_eq!(allocator.created.get(), 2);
}

#[test]
fn a_frame_larger_than_a_slot_may_hold_is_refused() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::with_limits(STAGING_SLOTS, 1 << 20);
    assert!(ring.acquire(&allocator, (1 << 20) + 1).is_none());
    assert_eq!(ring.stats().too_large, 1);
    assert_eq!(
        allocator.created.get(),
        0,
        "a refused frame allocates nothing"
    );
    // Exactly the ceiling still fits.
    let lease = ring.acquire(&allocator, 1 << 20).expect("at the ceiling");
    assert_eq!(lease.capacity(), 1 << 20);
}

#[test]
fn a_dropped_fence_abandons_its_slot_and_the_slot_is_replaced() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::with_limits(1, u64::MAX);

    // The encoder carrying the fence was dropped unsubmitted: its callback,
    // and the token in it, are dropped without ever running.
    drop(ring.acquire(&allocator, 1_000).expect("a slot").finish());
    assert_eq!(
        ring.slot(0).map(|(_, state)| state),
        Some(SlotState::Abandoned)
    );

    // Abandoned is not ready — no re-map is coming — so the slot is replaced
    // with a freshly mapped buffer rather than reused.
    let lease = ring.acquire(&allocator, 1_000).expect("a replaced slot");
    assert_eq!(lease.index(), 0);
    assert_eq!(allocator.created.get(), 2);
    lease.finish().signal(true);
    assert_eq!(ring.slot(0).map(|(_, state)| state), Some(SlotState::Ready));
}

#[test]
fn a_lease_dropped_unfinished_abandons_its_slot() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::with_limits(1, u64::MAX);
    drop(ring.acquire(&allocator, 1_000).expect("a slot"));
    assert_eq!(
        ring.slot(0).map(|(_, state)| state),
        Some(SlotState::Abandoned)
    );
}

#[test]
fn a_failed_remap_marks_the_slot_failed_and_it_is_replaced() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::with_limits(1, u64::MAX);
    ring.acquire(&allocator, 1_000)
        .expect("a slot")
        .finish()
        .signal(false);
    assert_eq!(
        ring.slot(0).map(|(_, state)| state),
        Some(SlotState::Failed)
    );
    let lease = ring.acquire(&allocator, 1_000).expect("a replaced slot");
    assert_eq!(lease.index(), 0);
    assert_eq!(allocator.created.get(), 2);
}

#[test]
fn reservations_are_aligned_and_refused_past_the_slot() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::new();
    let mut lease = ring.acquire(&allocator, 1_000).expect("a slot");

    assert_eq!(lease.reserve(3, BUFFER_COPY_ALIGNMENT), Some(0));
    assert_eq!(lease.reserve(32, BUFFER_COPY_ALIGNMENT), Some(16));
    assert_eq!(lease.reserve(10, TEXTURE_COPY_ALIGNMENT), Some(512));
    assert_eq!(lease.used(), 522);
    // Past the slot: nothing is placed and the cursor does not move.
    assert_eq!(lease.reserve(lease.capacity(), 4), None);
    assert_eq!(lease.used(), 522);
    // Exactly to the end still fits.
    let rest = lease.capacity() - 528;
    assert_eq!(lease.reserve(rest, 16), Some(528));
    assert_eq!(lease.used(), lease.capacity());

    assert_eq!(align_up(0, 16), Some(0));
    assert_eq!(align_up(17, 16), Some(32));
    assert_eq!(align_up(5, 0), Some(5), "a zero alignment is no alignment");
    assert_eq!(align_up(u64::MAX, 16), None);
    assert_eq!(slot_capacity(1), MIN_SLOT_BYTES);
    assert_eq!(slot_capacity(MIN_SLOT_BYTES + 1), MIN_SLOT_BYTES * 2);
}

/// A steady stream of frames through a ready slot allocates nothing in the
/// ring's own policy — the GPU-free half of the allocation evidence.
#[test]
fn a_steady_frame_allocates_nothing_in_the_ring() {
    let allocator = FakeAllocator::default();
    let mut ring: StagingRing<u32> = StagingRing::new();
    ring.acquire(&allocator, 4_000)
        .expect("a slot")
        .finish()
        .signal(true);
    let allocations = allocations_in(|| {
        for _ in 0..100 {
            let mut lease = ring.acquire(&allocator, 4_000).expect("the ready slot");
            let _ = lease.reserve(32, BUFFER_COPY_ALIGNMENT);
            let _ = lease.reserve(2_048, TEXTURE_COPY_ALIGNMENT);
            lease.finish().signal(true);
        }
    });
    assert_eq!(
        allocations, 0,
        "slot reuse, reservation and the fence allocate nothing"
    );
    assert_eq!(allocator.created.get(), 1);
}

// ---------------------------------------------------------------------------
// On a real device.
// ---------------------------------------------------------------------------

/// One device at a time, for the driver-teardown reason every GPU suite in
/// this workspace serializes on.
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `bytes` of `dst` read back through a mappable copy.
fn read_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    dst: &wgpu::Buffer,
    len: u64,
) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging test readback"),
        size: len,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(dst, 0, &readback, 0, len);
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback maps");
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let bytes = readback
        .slice(..)
        .get_mapped_range()
        .expect("the readback is mapped")
        .to_vec();
    readback.unmap();
    bytes
}

fn upload_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging test destination"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// A recognisable byte pattern that differs from frame to frame.
fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test staging -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn gpu_staged_copies_land_in_a_buffer_and_a_texture_exactly() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let (device, queue) = (&handle.device, &handle.queue);

        // A Rgba32Uint texture 64 texels wide: 1024 bytes a row, so the copy
        // needs no row padding — the shape the engine's resource textures take.
        let (width, rows) = (64_u32, 4_u32);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("staging test texture"),
            size: wgpu::Extent3d {
                width,
                height: rows,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Uint,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let bytes_per_row = width * 16;
        let texture_len = u64::from(bytes_per_row * rows);
        let dst = upload_buffer(device, 256);

        let mut ring = StagingRing::new();
        let small = pattern(36, 7);
        let tail = pattern(28, 9);
        let pixels = pattern(texture_len as usize, 3);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut frame = ring
            .begin(device, 4_096)
            .expect("a first frame gets a slot");
        assert!(frame.copy_parts_to_buffer(&mut encoder, &[&small, &tail], &dst, 64));
        assert!(frame.copy_to_texture(
            &mut encoder,
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes_per_row,
            rows,
            texture.size(),
            |mut dst| dst.copy_from_slice(&pixels),
        ));
        // A slice that is not a whole number of copy units is refused, so the
        // caller writes it by queue instead.
        assert!(!frame.copy_to_buffer(&mut encoder, &[1, 2, 3], &dst, 0));
        assert!(frame.used() >= texture_len + 64);
        frame.finish(&encoder);
        queue.submit([encoder.finish()]);

        let read = read_buffer(device, queue, &dst, 256);
        assert_eq!(&read[64..100], small.as_slice());
        assert_eq!(
            &read[100..128],
            tail.as_slice(),
            "the parts land back to back"
        );

        let readback = upload_buffer(device, texture_len);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(rows),
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        assert_eq!(read_buffer(device, queue, &readback, texture_len), pixels);
        assert_eq!(handle.first_uncaptured_error(), None);
    });
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test staging -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn gpu_a_slot_is_reused_once_its_submission_retires() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let (device, queue) = (&handle.device, &handle.queue);
        let dst = upload_buffer(device, 1_024);
        let mut ring = StagingRing::new();

        // Frames submitted back to back, never waited on: the ring never
        // holds more than its slots, and each frame's bytes are the ones that
        // land (a slot rewritten while the GPU still read it would show an
        // earlier or later frame's pattern instead).
        for frame_index in 0..24_u8 {
            let bytes = pattern(512, frame_index);
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            match ring.begin(device, 1_024) {
                Some(mut frame) => {
                    assert!(frame.copy_to_buffer(&mut encoder, &bytes, &dst, 0));
                    frame.finish(&encoder);
                }
                None => queue.write_buffer(&dst, 0, &bytes),
            }
            queue.submit([encoder.finish()]);
            assert!(ring.slot_count() <= STAGING_SLOTS);
            if frame_index % 6 == 5 {
                assert_eq!(read_buffer(device, queue, &dst, 512), bytes);
            }
        }
        let stats = ring.stats();
        println!("staging ring after 24 unpaced frames: {stats:?}");
        assert!(stats.frames >= 3, "the ring served frames: {stats:?}");

        // Paced frames — each one's submission retired before the next — all
        // land in the same slot, and no buffer beyond the first is created.
        let created = ring.stats().buffers_created;
        for frame_index in 0..10_u8 {
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let mut frame = ring.begin(device, 1_024).expect("a retired slot is ready");
            assert!(frame.copy_to_buffer(&mut encoder, &pattern(512, frame_index), &dst, 0));
            frame.finish(&encoder);
            queue.submit([encoder.finish()]);
        }
        assert_eq!(
            ring.stats().buffers_created,
            created,
            "paced frames reuse slots"
        );
        assert_eq!(handle.first_uncaptured_error(), None);
    });
}

#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test staging -- --ignored \
            --test-threads=1` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn gpu_a_dropped_encoder_abandons_its_slot_and_the_ring_recovers() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let (device, queue) = (&handle.device, &handle.queue);
        let dst = upload_buffer(device, 256);
        let mut ring = StagingRing::with_limits(1, 1 << 20);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut frame = ring.begin(device, 256).expect("a slot");
        assert!(frame.copy_to_buffer(&mut encoder, &pattern(64, 1), &dst, 0));
        frame.finish(&encoder);
        // Never submitted: the deferred re-map and its fence go with it.
        drop(encoder);
        assert_eq!(
            ring.slot(0).map(|(_, state)| state),
            Some(SlotState::Abandoned)
        );

        let bytes = pattern(64, 2);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut frame = ring
            .begin(device, 256)
            .expect("the abandoned slot is replaced");
        assert!(frame.copy_to_buffer(&mut encoder, &bytes, &dst, 0));
        frame.finish(&encoder);
        queue.submit([encoder.finish()]);
        assert_eq!(read_buffer(device, queue, &dst, 64), bytes);
        assert_eq!(ring.stats().buffers_created, 2);

        // A frame dropped without finishing still unmaps its slot, so the
        // encoder holding its copy can be submitted.
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let bytes = pattern(64, 3);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut frame = ring.begin(device, 256).expect("the retired slot");
        assert!(frame.copy_to_buffer(&mut encoder, &bytes, &dst, 0));
        drop(frame);
        queue.submit([encoder.finish()]);
        assert_eq!(read_buffer(device, queue, &dst, 64), bytes);
        assert_eq!(handle.first_uncaptured_error(), None);
    });
}

/// The allocation evidence: the heap allocations of one steady-state frame
/// that uploads five buffers' worth of data, staged through the ring against
/// the same five uploads as queue writes. Both arms create, record into and
/// submit one encoder; only the upload route differs.
#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu --test staging -- --ignored \
            --test-threads=1 --nocapture` (pin the adapter with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn gpu_a_staged_frame_allocates_no_more_than_queue_writes() {
    let _serialized = gpu_lock();
    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        let (device, queue) = (&handle.device, &handle.queue);
        let sizes = [32_usize, 4_096, 32, 65_536, 128];
        let destinations: Vec<wgpu::Buffer> = sizes
            .iter()
            .map(|size| upload_buffer(device, *size as u64))
            .collect();
        let payloads: Vec<Vec<u8>> = sizes.iter().map(|size| pattern(*size, 5)).collect();
        let mut ring = StagingRing::new();

        let staged_frame = |ring: &mut StagingRing| {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let mut frame = ring.begin(device, 128 * 1024).expect("a ready slot");
            for (bytes, dst) in payloads.iter().zip(&destinations) {
                assert!(frame.copy_to_buffer(&mut encoder, bytes, dst, 0));
            }
            frame.finish(&encoder);
            queue.submit([encoder.finish()]);
        };
        let queued_frame = || {
            let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (bytes, dst) in payloads.iter().zip(&destinations) {
                queue.write_buffer(dst, 0, bytes);
            }
            queue.submit([encoder.finish()]);
        };

        // Warm both routes (slot creation, wgpu's own pools), then count a
        // run of paced frames of each.
        const FRAMES: u64 = 30;
        for _ in 0..4 {
            staged_frame(&mut ring);
            queued_frame();
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        let mut staged = 0;
        let mut queued = 0;
        for _ in 0..FRAMES {
            staged += allocations_in(|| staged_frame(&mut ring));
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            queued += allocations_in(queued_frame);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        let (staged, queued) = (staged / FRAMES, queued / FRAMES);
        println!(
            "heap allocations per frame (5 uploads): staged {staged}, queue writes {queued}; \
             ring {:?}",
            ring.stats()
        );
        assert!(
            staged <= queued,
            "a staged frame allocates {staged} times, queue writes {queued}"
        );
        assert_eq!(
            ring.stats().busy,
            0,
            "paced frames never find the ring busy"
        );
        assert_eq!(handle.first_uncaptured_error(), None);
    });
}
