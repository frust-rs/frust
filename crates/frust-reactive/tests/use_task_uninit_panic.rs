//! Integration test for `use_task` panic on uninitialized runtime.
//!
//! This test runs in a fresh process (no process-wide ReactiveRuntime
//! initialized), mounts a component using `use_task`, and expects a panic
//! with the wiring-bug message.

use frust_reactive::use_task;
use reactive_graph::owner::Owner;
use std::io;

#[test]
#[should_panic(expected = "wiring bug")]
fn use_task_panics_on_uninitialized_runtime() {
    // No ReactiveRuntime::init() call — this is a fresh process with no
    // runtime installed. Build an Owner and call use_task inside it; the
    // panic should fire on the first call when run_once tries to fetch the
    // runtime.
    let owner = Owner::new();
    owner.with(|| {
        let _task = use_task(|| async {
            // This closure never runs; the panic fires in run_once before
            // spawning any background work.
            Result::<i32, io::Error>::Ok(42)
        });
    });
}
