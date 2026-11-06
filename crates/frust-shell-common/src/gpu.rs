//! The process-wide, replaceable GPU-device slot: [`GpuPublisher`] writes it,
//! [`with_gpu_handle`] and [`gpu_handle_generation`] read it.
//!
//! # The gap this closes
//!
//! Every shell creates its GPU device lazily, on its own thread, the first
//! time a surface comes up (`frust_render::RenderContext`, owned wholly by
//! the shell's render executor). A shell can also drop and recreate that
//! device (an Android Activity destroy/recreate builds a fresh render
//! context). This module is the hand-back seam's shared slot: a shell
//! publishes its live device from wherever it creates one, and app code —
//! through the facade's `frust::gpu::with_context` — reads it back without
//! the facade depending on `frust-gpu`/`frust-render` for this seam's sake.
//!
//! # Replaceable and generation-tagged
//!
//! The slot holds at most one installed handle, tagged with the device
//! *generation* its publisher reported (`DeviceHandle::generation` in
//! `frust-render`) and a process-unique token naming the publisher install.
//! A later publish replaces the earlier one (last writer wins), so a rebuilt
//! device becomes visible to readers as soon as its owner publishes it.
//! Readers detect the swap through [`gpu_handle_generation`].
//!
//! # Compare-and-clear
//!
//! A [`GpuPublisher`] clears the slot on [`GpuPublisher::withdraw`] and on
//! `Drop`, but only if the slot still holds *its own* token. An older
//! executor dying late therefore never clears a newer device published by
//! its successor.
//!
//! # Type-erased on purpose
//!
//! `frust-shell-common` depends on none of `frust-gpu`/`frust-render`/`wgpu`
//! and this seam adds no edge to change that: the slot stores an opaque
//! `Arc<dyn Any + Send + Sync>`. Every real caller instantiates the generics
//! with the one concrete device-handle type. Reading with a mismatched `T`
//! answers `None`, exactly like reading an empty slot.
//!
//! # Who may publish
//!
//! Only a shell's render executor, at the point it has a live device: desktop
//! (`frust-shell-desktop::render`, after each successful surface install),
//! Android and iOS (their render executors, once the surface's device is up). The
//! function is `pub` so any shell crate can publish; app code reads through
//! the facade and never publishes.
//!
//! # Thread contract
//!
//! Every function is callable from any thread. The slot is a `Mutex`
//! (poisoning ignored) held only long enough to clone or swap an `Arc`. The
//! one piece of caller code that runs under it is [`GpuPublisher::publish`]'s
//! `make`, which must therefore be cheap and must not touch the slot (a shell
//! only clones its handle). A reader's closure and a displaced handle's `Drop`
//! both run after the lock is released, so [`with_gpu_handle`] may be called
//! re-entrantly from either. A read may wait for an in-flight publish, never
//! for a frame.

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

struct Installed {
    token: u64,
    generation: u64,
    handle: Arc<dyn Any + Send + Sync>,
}

static SLOT: Mutex<Option<Installed>> = Mutex::new(None);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn lock_slot() -> MutexGuard<'static, Option<Installed>> {
    SLOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One device owner's write access to the slot. See the module docs.
#[derive(Debug, Default)]
pub struct GpuPublisher {
    /// Token and generation of this publisher's most recent install.
    last: Option<(u64, u64)>,
}

impl GpuPublisher {
    /// A publisher that has installed nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish `make()` as the live device handle for `generation`.
    ///
    /// Installs (last writer wins) only if `generation` differs from what
    /// this publisher last installed, or the slot no longer holds this
    /// publisher's token (another publisher displaced it, or it was
    /// withdrawn). Otherwise a no-op and `make` is not called, so a shell may
    /// call this after every surface install cheaply.
    ///
    /// `make` runs while the slot lock is held, so it must be cheap (the
    /// desktop shell only clones a handle) and must not touch the slot
    /// (`with_gpu_handle`, `gpu_handle_generation` or another publish would
    /// deadlock). The displaced previous handle, by contrast, is dropped only
    /// after the lock is released, so its `Drop` may read the slot.
    pub fn publish<T>(&mut self, generation: u64, make: impl FnOnce() -> T)
    where
        T: Any + Send + Sync,
    {
        let mut slot = lock_slot();
        if let Some((token, last_gen)) = self.last {
            let still_ours = slot.as_ref().is_some_and(|i| i.token == token);
            if still_ours && last_gen == generation {
                return;
            }
        }
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let old = slot.replace(Installed {
            token,
            generation,
            handle: Arc::new(make()),
        });
        drop(slot);
        // Drop the displaced handle after releasing the lock: its Drop may
        // run arbitrary code.
        drop(old);
        self.last = Some((token, generation));
    }

    /// Clear the slot if it still holds this publisher's install.
    pub fn withdraw(&mut self) {
        if let Some((token, _)) = self.last.take() {
            let mut slot = lock_slot();
            if slot.as_ref().is_some_and(|i| i.token == token) {
                // Drop the old handle after releasing the lock: its Drop may
                // run arbitrary code.
                let old = slot.take();
                drop(slot);
                drop(old);
            }
        }
    }
}

impl Drop for GpuPublisher {
    fn drop(&mut self) {
        self.withdraw();
    }
}

/// Run `f` against the currently published handle, `None` if nothing is
/// published or `T` does not match the published type.
///
/// The handle is current only for the call; do not stash clones across
/// frames. The slot lock is released before `f` runs.
pub fn with_gpu_handle<T, R>(f: impl FnOnce(&T) -> R) -> Option<R>
where
    T: Any + Send + Sync,
{
    let handle = lock_slot().as_ref().map(|i| Arc::clone(&i.handle))?;
    let typed = handle.downcast_ref::<T>()?;
    Some(f(typed))
}

/// The generation of the currently published handle, `None` if empty.
pub fn gpu_handle_generation() -> Option<u64> {
    lock_slot().as_ref().map(|i| i.generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot is process-wide, so every test serializes on this lock.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[derive(Debug, PartialEq)]
    struct Dev(u32);

    fn read() -> Option<u32> {
        with_gpu_handle::<Dev, _>(|d| d.0)
    }

    #[test]
    fn last_publish_wins() {
        let _g = serial();
        let mut a = GpuPublisher::new();
        let mut b = GpuPublisher::new();
        a.publish(1, || Dev(1));
        b.publish(5, || Dev(2));
        assert_eq!(read(), Some(2));
        assert_eq!(gpu_handle_generation(), Some(5));
    }

    #[test]
    fn displaced_publisher_does_not_clear_newer_install() {
        let _g = serial();
        let mut a = GpuPublisher::new();
        let mut b = GpuPublisher::new();
        a.publish(1, || Dev(1));
        b.publish(2, || Dev(2));
        drop(a);
        assert_eq!(read(), Some(2));
        drop(b);
        assert_eq!(read(), None);
        assert_eq!(gpu_handle_generation(), None);
    }

    #[test]
    fn publisher_clears_its_own_on_drop_and_withdraw() {
        let _g = serial();
        let mut a = GpuPublisher::new();
        a.publish(1, || Dev(1));
        assert_eq!(read(), Some(1));
        a.withdraw();
        assert_eq!(read(), None);
        a.publish(2, || Dev(3));
        assert_eq!(read(), Some(3));
        drop(a);
        assert_eq!(read(), None);
    }

    #[test]
    fn republishing_the_same_generation_keeps_the_token() {
        let _g = serial();
        let mut a = GpuPublisher::new();
        a.publish(4, || Dev(1));
        let token = a.last.unwrap().0;
        a.publish(4, || -> Dev { panic!("make must not run") });
        assert_eq!(a.last.unwrap().0, token);
        assert_eq!(read(), Some(1));
        a.publish(5, || Dev(9));
        assert_ne!(a.last.unwrap().0, token);
        assert_eq!(read(), Some(9));
    }

    #[test]
    fn republish_after_displacement_reinstalls() {
        let _g = serial();
        let mut a = GpuPublisher::new();
        let mut b = GpuPublisher::new();
        a.publish(1, || Dev(1));
        b.publish(1, || Dev(2));
        a.publish(1, || Dev(3));
        assert_eq!(read(), Some(3));
    }

    #[test]
    fn reentrant_read_does_not_deadlock() {
        let _g = serial();
        let mut a = GpuPublisher::new();
        a.publish(1, || Dev(7));
        let inner = with_gpu_handle::<Dev, _>(|_| (read(), gpu_handle_generation()));
        assert_eq!(inner, Some((Some(7), Some(1))));
    }

    #[test]
    fn displaced_handle_may_read_the_slot_from_its_drop() {
        let _g = serial();
        struct ReadsOnDrop;
        impl Drop for ReadsOnDrop {
            fn drop(&mut self) {
                let _ = with_gpu_handle::<Dev, _>(|d| d.0);
                let _ = gpu_handle_generation();
            }
        }
        let mut a = GpuPublisher::new();
        a.publish(1, || ReadsOnDrop);
        let mut b = GpuPublisher::new();
        // Displaces `a`'s handle; its Drop re-enters the slot.
        b.publish(2, || Dev(2));
        assert_eq!(read(), Some(2));
    }

    #[test]
    fn mismatched_type_answers_none() {
        let _g = serial();
        struct Other;
        let mut a = GpuPublisher::new();
        a.publish(1, || Dev(1));
        assert!(with_gpu_handle::<Other, _>(|_| ()).is_none());
    }

    #[test]
    fn readers_racing_installs_never_panic() {
        let _g = serial();
        let readers: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..1000 {
                        let _ = with_gpu_handle::<Dev, _>(|d| d.0);
                        let _ = gpu_handle_generation();
                    }
                })
            })
            .collect();
        let mut a = GpuPublisher::new();
        for g in 0..500u64 {
            a.publish(g, || Dev(g as u32));
            if g % 7 == 0 {
                a.withdraw();
            }
        }
        for r in readers {
            r.join().expect("reader thread must not panic");
        }
    }
}
