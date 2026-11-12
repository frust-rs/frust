//! Under `test-support`, lifecycle listeners are delivered per creating thread, so tests on
//! separate threads do not see each other's pushes.
#![cfg(feature = "test-support")]

use std::sync::Arc;
use std::sync::Barrier;
use std::sync::atomic::{AtomicUsize, Ordering};

use frust::{AppLifecycleListener, MemoryPressure, push_memory_pressure};
use frust_reactive::ReactiveRuntime;

fn counting_listener() -> (AppLifecycleListener, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let listener = AppLifecycleListener::new().on_memory_pressure(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    (listener, count)
}

#[test]
fn concurrent_threads_each_hear_only_their_own_push() {
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                // Marks this thread as the UI thread, as `frust::testing::Tester` does.
                ReactiveRuntime::init(Arc::new(|| {}));
                let (_listener, count) = counting_listener();
                barrier.wait();
                push_memory_pressure(MemoryPressure::Moderate);
                barrier.wait();
                count.load(Ordering::SeqCst)
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), 1);
    }
}

#[test]
fn listener_does_not_fire_for_a_push_from_another_thread() {
    let (_listener, count) = counting_listener();
    let pusher = std::thread::spawn(|| {
        ReactiveRuntime::init(Arc::new(|| {}));
        push_memory_pressure(MemoryPressure::Critical);
    });
    pusher.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
