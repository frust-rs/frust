//! Behavioral tests for the texture pool and the per-frame host arena.
//!
//! Every test but the last runs with no GPU: `TexturePool` allocates through
//! a `TextureAllocator` and `HostBuffer` uploads through a `BufferUploader`,
//! so counting fakes answer the two questions that matter — how many
//! textures a resize storm actually allocates, and how many buffers a steady
//! frame loop creates — on any host. The single `#[ignore]` test at the end
//! puts both types on a real device.

use std::cell::Cell;
use std::collections::HashSet;

use frust_gpu::arena::{BufferSlice, BufferUploader, HostBuffer};
use frust_gpu::caps::{DownlevelProfile, TierCaps};
use frust_gpu::pool::{SIZE_QUANTUM, TextureAllocator, TexturePool};
use frust_gpu::texture::TextureDesc;

/// Counts texture allocations and hands back plain ids in place of a
/// `wgpu::Texture`/`wgpu::TextureView` pair.
#[derive(Debug, Default)]
struct FakeTextureAllocator {
    created: Cell<u32>,
}

impl TextureAllocator for FakeTextureAllocator {
    type Texture = u32;
    type View = u32;

    fn allocate_texture(&self, _desc: &TextureDesc) -> (u32, u32) {
        self.created.set(self.created.get() + 1);
        (self.created.get(), self.created.get())
    }
}

/// Counts buffer creations and uploads, so "did the second frame reuse the
/// same buffer?" is a plain assertion.
#[derive(Debug, Default)]
struct FakeUploader {
    created: Cell<u32>,
    writes: Cell<u32>,
}

impl BufferUploader for FakeUploader {
    type Buffer = u32;

    fn allocate_buffer(&self, _label: Option<&str>, _size: u64, _usage: wgpu::BufferUsages) -> u32 {
        self.created.set(self.created.get() + 1);
        self.created.get()
    }

    fn write_buffer(&self, _buffer: &u32, _offset: u64, _bytes: &[u8]) {
        self.writes.set(self.writes.get() + 1);
    }
}

/// A deterministic xorshift, so a "pseudo-random" storm is the same storm on
/// every run and every host — a test that allocated a different number of
/// textures each run could not hold a budget.
struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 33) as u32) % bound
    }
}

fn attachment(width: u32, height: u32) -> TextureDesc {
    TextureDesc {
        width,
        height,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        label: Some("intermediate".to_string()),
    }
}

fn desktop_caps() -> TierCaps {
    TierCaps::fake(DownlevelProfile::Full)
}

/// The narrow end of the resize storm: an 800x800 window.
const STORM_MIN_WIDTH: u32 = 800;
/// The wide end: a 5120x2880 window filling a 5K display.
const STORM_MAX_WIDTH: u32 = 5120;

/// One resize storm: 400 frames of a corner drag between 800x800 and
/// 5120x2880, as `(width, height)` pairs.
///
/// The drag is modelled as a pointer with momentum — a pseudo-random
/// acceleration each frame, a capped speed, and a bounce off each end of the
/// range. That, not a per-frame teleport to an unrelated size, is what a
/// resize storm is: a hand moving a corner, so consecutive frames are
/// correlated. The window is aspect-locked to the 16:9 display it is being
/// dragged on and floored at 800x800.
fn resize_storm_trace() -> Vec<(u32, u32)> {
    const FRAMES: usize = 400;
    const MIN_WIDTH: i64 = STORM_MIN_WIDTH as i64;
    const MAX_WIDTH: i64 = STORM_MAX_WIDTH as i64;
    const MAX_SPEED: i64 = 72;

    let mut rng = Rng(0x5eed_1234_9e37_79b9);
    let mut width = MIN_WIDTH;
    let mut velocity = MAX_SPEED / 2;
    let mut trace = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        velocity = (velocity + i64::from(rng.below(25)) - 12).clamp(-MAX_SPEED, MAX_SPEED);
        width += velocity;
        if width < MIN_WIDTH {
            width = MIN_WIDTH;
            velocity = -velocity;
        } else if width > MAX_WIDTH {
            width = MAX_WIDTH;
            velocity = -velocity;
        }
        let width = u32::try_from(width).expect("the drag stays inside the range");
        trace.push((width, (width * 9 / 16).max(STORM_MIN_WIDTH)));
    }
    trace
}

/// The trace itself: it must sweep the whole range and contain hundreds of
/// distinct exact sizes, or the pool tests below would be measuring a drag
/// that never went anywhere.
#[test]
fn the_resize_storm_covers_the_range_it_claims_to() {
    let trace = resize_storm_trace();
    let widths: Vec<u32> = trace.iter().map(|(width, _)| *width).collect();
    let narrowest = *widths.iter().min().expect("a non-empty trace");
    let widest = *widths.iter().max().expect("a non-empty trace");
    let exact: HashSet<(u32, u32)> = trace.iter().copied().collect();

    assert_eq!(trace.len(), 400);
    assert!(
        narrowest <= STORM_MIN_WIDTH + SIZE_QUANTUM && widest >= STORM_MAX_WIDTH - SIZE_QUANTUM,
        "the drag must sweep the whole range; it covered {narrowest}..{widest}"
    );
    assert!(
        exact.len() > 100,
        "the storm must contain many distinct exact sizes for the \
         quantization to be what collapses them; saw {}",
        exact.len()
    );
}

/// The headline case: 400 resizes collapse onto at most 40 distinct pooled
/// textures instead of 400.
///
/// Aging is on but its window is opened wide, which isolates what the
/// quantization alone buys: every frame acquires, releases and ages, and the
/// pool still allocates once per distinct quantized configuration. Hundreds
/// of distinct exact sizes (asserted above) become a couple of dozen
/// textures.
#[test]
fn a_resize_storm_collapses_onto_few_pooled_textures() {
    let caps = desktop_caps();
    let allocator = FakeTextureAllocator::default();
    let mut pool: TexturePool<u32, u32> = TexturePool::with_max_unused_frames(&caps, u64::MAX);
    let mut configurations: HashSet<_> = HashSet::new();

    for (frame, (width, height)) in resize_storm_trace().into_iter().enumerate() {
        let desc = attachment(width, height);
        configurations.insert(pool.key_for(&desc));

        let texture = pool.acquire(&allocator, &desc, frame as u64);
        let (pooled_width, pooled_height) = texture.size();
        assert!(
            pooled_width >= width && pooled_height >= height,
            "a pooled texture is never smaller than the request: \
             {pooled_width}x{pooled_height} for {width}x{height}"
        );
        assert_eq!(texture.requested_size(), (width, height));
        pool.release(texture);
        pool.age(frame as u64);
    }

    let stats = pool.stats();
    assert!(
        stats.created <= 40,
        "400 resizes allocated {} textures (reused {})",
        stats.created,
        stats.reused
    );
    assert_eq!(
        stats.created,
        configurations.len() as u64,
        "with nothing evicted, one allocation per distinct configuration"
    );
    assert_eq!(stats.evicted, 0);
    assert_eq!(stats.in_use, 0, "every acquire was released");
    assert_eq!(allocator.created.get() as u64, stats.created);
}

/// The same storm under the default 60-frame keep-alive window, which is
/// where the second cost lives: a full-range drag leaves a size behind for
/// longer than the window, so the entry is evicted and the return sweep
/// allocates it again.
///
/// Reuse still dominates by a wide margin — that is the property worth
/// holding — and the extra allocations are the window's price, not the
/// quantization's, which is why the window is a constructor parameter. A
/// host that would rather pay memory than allocations widens it; the
/// two-frame window `frust-render`'s compositor ages its scratch by would
/// make this storm allocate nearly every frame.
#[test]
fn the_keep_alive_window_is_what_costs_the_extra_allocations() {
    let caps = desktop_caps();
    let allocator = FakeTextureAllocator::default();
    let mut pool: TexturePool<u32, u32> = TexturePool::new(&caps);
    assert_eq!(pool.max_unused_frames(), 60);

    for (frame, (width, height)) in resize_storm_trace().into_iter().enumerate() {
        let texture = pool.acquire(&allocator, &attachment(width, height), frame as u64);
        pool.release(texture);
        pool.age(frame as u64);
    }

    let stats = pool.stats();
    assert!(
        stats.reused >= 4 * stats.created,
        "reuse must dominate: {} reused against {} allocated ({} evicted)",
        stats.reused,
        stats.created,
        stats.evicted
    );
    assert!(
        stats.created <= 80,
        "400 resizes allocated {} textures ({} of them replacing an evicted \
         entry)",
        stats.created,
        stats.evicted
    );
    assert!(
        stats.evicted > 0,
        "the 60-frame window must have evicted something over a 400-frame drag"
    );
}

/// Every allocation is rounded up to the next 256-px multiple in both axes,
/// which is what makes two nearby requests share one texture.
#[test]
fn acquire_quantizes_the_extent_up() {
    let caps = desktop_caps();
    let allocator = FakeTextureAllocator::default();
    let mut pool: TexturePool<u32, u32> = TexturePool::new(&caps);

    let first = pool.acquire(&allocator, &attachment(801, 513), 0);
    assert_eq!(first.size(), (1024, 768));
    assert_eq!(first.size().0 % SIZE_QUANTUM, 0);
    assert_eq!(first.size().1 % SIZE_QUANTUM, 0);
    pool.release(first);

    // A different request inside the same quantum reuses that texture.
    let second = pool.acquire(&allocator, &attachment(1024, 700), 1);
    assert_eq!(second.size(), (1024, 768));
    assert_eq!(second.requested_size(), (1024, 700));
    pool.release(second);

    // One quantum further out is a different key, so it allocates.
    let third = pool.acquire(&allocator, &attachment(1025, 700), 2);
    assert_eq!(third.size(), (1280, 768));
    pool.release(third);

    let stats = pool.stats();
    assert_eq!(stats.created, 2);
    assert_eq!(stats.reused, 1);
    assert_eq!(allocator.created.get(), 2);
}

/// Format, usage and quantized size each key separately: a texture is never
/// handed to a request it does not actually fit.
#[test]
fn configurations_never_share_a_texture() {
    let caps = desktop_caps();
    let allocator = FakeTextureAllocator::default();
    let mut pool: TexturePool<u32, u32> = TexturePool::new(&caps);

    let base = attachment(512, 512);
    let other_format = TextureDesc {
        format: wgpu::TextureFormat::Rgba16Float,
        ..base.clone()
    };
    let other_usage = TextureDesc {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        ..base.clone()
    };

    for (frame, desc) in [&base, &other_format, &other_usage].into_iter().enumerate() {
        let texture = pool.acquire(&allocator, desc, frame as u64);
        pool.release(texture);
    }
    assert_eq!(pool.stats().created, 3);
    assert_eq!(pool.stats().keys, 3);

    // Each of the three now reuses its own entry.
    for (frame, desc) in [&base, &other_format, &other_usage].into_iter().enumerate() {
        let texture = pool.acquire(&allocator, desc, 10 + frame as u64);
        pool.release(texture);
    }
    assert_eq!(pool.stats().created, 3);
    assert_eq!(pool.stats().reused, 3);
}

/// A parked entry survives 60 idle frames and is dropped on the 61st, so a
/// drag that revisits a size a second later still finds it.
#[test]
fn aging_frees_an_entry_after_the_keep_alive_window() {
    let caps = desktop_caps();
    let allocator = FakeTextureAllocator::default();
    let mut pool: TexturePool<u32, u32> = TexturePool::new(&caps);
    assert_eq!(pool.max_unused_frames(), 60);

    let texture = pool.acquire(&allocator, &attachment(1024, 1024), 0);
    pool.release(texture);

    for frame in 1..=60 {
        pool.age(frame);
    }
    assert_eq!(pool.stats().free, 1, "60 idle frames must not evict");
    assert_eq!(pool.stats().evicted, 0);

    pool.age(61);
    let stats = pool.stats();
    assert_eq!(stats.free, 0);
    assert_eq!(stats.keys, 0);
    assert_eq!(stats.evicted, 1);

    // The next request for that size has to allocate again.
    let texture = pool.acquire(&allocator, &attachment(1024, 1024), 62);
    assert_eq!(pool.stats().created, 2);
    pool.release(texture);
}

/// The transient policy: a write-only attachment gets
/// `TextureUsages::TRANSIENT_ATTACHMENT` on an adapter that benefits, and
/// nothing that is read back ever does.
#[test]
fn transient_usage_follows_the_adapter_and_the_usage_set() {
    let mut transient_caps = desktop_caps();
    transient_caps.transient_saves_memory = true;
    let allocator = FakeTextureAllocator::default();
    let mut pool: TexturePool<u32, u32> = TexturePool::new(&transient_caps);

    let write_only = pool.acquire(&allocator, &attachment(256, 256), 0);
    assert!(
        write_only
            .texture()
            .desc()
            .usage
            .contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT)
    );
    pool.release(write_only);

    let sampled_desc = TextureDesc {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        ..attachment(256, 256)
    };
    let sampled = pool.acquire(&allocator, &sampled_desc, 1);
    assert!(
        !sampled
            .texture()
            .desc()
            .usage
            .contains(wgpu::TextureUsages::TRANSIENT_ATTACHMENT),
        "a texture that is sampled afterwards is read back, so it is not transient"
    );
    pool.release(sampled);

    // An adapter that reports no benefit gets the flag nowhere.
    let plain_caps = desktop_caps();
    assert!(!plain_caps.transient_saves_memory);
    let mut plain_pool: TexturePool<u32, u32> = TexturePool::new(&plain_caps);
    let plain = plain_pool.acquire(&allocator, &attachment(256, 256), 0);
    assert_eq!(
        plain.texture().desc().usage,
        wgpu::TextureUsages::RENDER_ATTACHMENT
    );
    plain_pool.release(plain);
}

/// Arena offsets are aligned to the adapter's uniform-offset minimum (256 on
/// the strictest profile), and the slices never overlap.
#[test]
fn arena_offsets_are_aligned_and_disjoint() {
    let caps = TierCaps::fake(DownlevelProfile::WebGl2);
    assert_eq!(caps.min_uniform_buffer_offset_alignment, 256);
    let mut arena: HostBuffer<u32> = HostBuffer::new(&caps);
    let mut rng = Rng(0x1234_5678_9abc_def1);

    let mut previous = BufferSlice::default();
    for index in 0..200u32 {
        let size = 1 + rng.below(1024) as usize;
        let slice = arena.alloc(&vec![u8::try_from(index % 251).unwrap(); size], 1);

        assert_eq!(
            slice.offset % 256,
            0,
            "offset {} is not 256-aligned",
            slice.offset
        );
        assert_eq!(slice.size, size as u64);
        if index > 0 {
            assert!(
                slice.offset >= previous.offset + previous.size,
                "slice {slice:?} overlaps {previous:?}"
            );
        }
        previous = slice;
    }
}

/// A second frame rewinds and refills the arena without creating a second
/// GPU buffer, and each frame costs exactly one upload.
#[test]
fn a_second_frame_reuses_the_same_buffer() {
    let caps = TierCaps::fake(DownlevelProfile::WebGl2);
    let uploader = FakeUploader::default();
    let mut arena: HostBuffer<u32> = HostBuffer::new(&caps);

    arena.alloc(&[1u8; 64], 1);
    arena.alloc(&[2u8; 300], 1);
    let first = arena
        .flush(&uploader)
        .copied()
        .expect("first frame flushed");
    let capacity = arena.capacity();

    arena.reset();
    assert_eq!(arena.used(), 0);
    arena.alloc(&[3u8; 64], 1);
    arena.alloc(&[4u8; 300], 1);
    let second = arena
        .flush(&uploader)
        .copied()
        .expect("second frame flushed");

    assert_eq!(first, second, "the second frame must reuse the same buffer");
    assert_eq!(arena.capacity(), capacity, "no growth for the same payload");
    assert_eq!(uploader.created.get(), 1);
    assert_eq!(arena.buffer_allocations(), 1);
    assert_eq!(uploader.writes.get(), 2, "one upload per frame");
    assert_eq!(arena.flush_count(), 2);
    assert_eq!(arena.high_water_mark(), 556);
}

/// The arena grows to fit a bigger frame, keeps that capacity for the small
/// frames that follow, and reports the largest frame it has held.
#[test]
fn arena_grows_only_and_keeps_its_high_water_mark() {
    let caps = TierCaps::fake(DownlevelProfile::WebGl2);
    let uploader = FakeUploader::default();
    let mut arena: HostBuffer<u32> = HostBuffer::new(&caps);

    arena.alloc(&[0u8; 128], 1);
    arena.flush(&uploader);
    let initial_capacity = arena.capacity();
    assert!(initial_capacity >= 128);

    arena.reset();
    let big = vec![0u8; 4 * initial_capacity as usize];
    arena.alloc(&big, 1);
    arena.flush(&uploader);
    let grown_capacity = arena.capacity();
    assert!(grown_capacity >= big.len() as u64);
    assert_eq!(uploader.created.get(), 2, "growth creates one new buffer");

    arena.reset();
    arena.alloc(&[0u8; 16], 1);
    arena.flush(&uploader);
    assert_eq!(
        arena.capacity(),
        grown_capacity,
        "a small frame never shrinks the buffer"
    );
    assert_eq!(uploader.created.get(), 2);
    assert_eq!(arena.high_water_mark(), big.len() as u64);
}

/// Both types against a real adapter: the pool creates and recycles an
/// actual `wgpu::Texture` at the quantized size, and the arena creates and
/// writes an actual `wgpu::Buffer` — the host tests above prove the policy,
/// this proves the descriptors `wgpu` is handed are ones it accepts.
#[test]
#[ignore = "needs a real GPU adapter; run with `cargo test -p frust-gpu -- --ignored` \
            (pin the adapter on a multi-GPU host with WGPU_BACKEND / WGPU_ADAPTER_NAME)"]
fn gpu_pool_and_arena_round_trip_on_a_real_device() {
    use frust_gpu::context::RenderContext;

    pollster::block_on(async {
        let mut context = RenderContext::new();
        let handle = context.device().await.expect("device creation");
        println!(
            "frust-gpu pool/arena adapter: {:?}",
            handle.adapter.get_info()
        );

        let mut pool: TexturePool = TexturePool::new(&handle.caps);
        let first = pool.acquire(&handle.device, &attachment(801, 513), 0);
        assert_eq!(first.size(), (1024, 768));
        let color = first.color_attachment(wgpu::Color::TRANSPARENT);
        assert_eq!(color.store, wgpu::StoreOp::Discard);
        pool.release(first);

        let second = pool.acquire(&handle.device, &attachment(900, 600), 1);
        assert_eq!(second.size(), (1024, 768));
        pool.release(second);
        assert_eq!(pool.stats().created, 1, "the second acquire must recycle");

        let mut arena: HostBuffer = HostBuffer::new(&handle.caps);
        arena.alloc(&[1u8; 64], 1);
        let slice = arena.alloc(&[2u8; 96], 1);
        assert_eq!(
            slice.offset % u64::from(handle.caps.min_uniform_buffer_offset_alignment),
            0
        );
        let buffer = arena.flush(handle).expect("flushed").clone();
        assert!(buffer.size() >= arena.used());

        arena.reset();
        arena.alloc(&[3u8; 64], 1);
        arena.flush(handle);
        assert_eq!(arena.buffer_allocations(), 1, "the second frame reuses it");

        let _ = handle.device.poll(wgpu::PollType::wait_indefinitely());
        assert_eq!(
            handle.first_uncaptured_error(),
            None,
            "the pool/arena descriptors must be ones wgpu accepts"
        );
    });
}
