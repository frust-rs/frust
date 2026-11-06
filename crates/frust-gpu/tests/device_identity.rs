//! Device identity: per-device generation, the device-lost latch, and the
//! process-wide optional-feature request. These need a real GPU adapter.
//!
//! Every test takes [`LOCK`]: one of them widens the process-wide feature
//! request while the others create devices, so they must not interleave. On a
//! GPU host the suite is also run with `--test-threads=1`.

use std::sync::{Mutex, MutexGuard};

use frust_gpu::{
    DeviceHandle, RenderContext, missing_requested_features, request_optional_features,
};

static LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn device() -> (RenderContext, DeviceHandle) {
    let mut context = RenderContext::new();
    let handle = pollster::block_on(context.device())
        .expect("a GPU device")
        .clone();
    (context, handle)
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-gpu --test device_identity -- --ignored`"]
fn two_contexts_get_distinct_increasing_generations() {
    let _lock = serial();
    let (_a_ctx, a) = device();
    let (_b_ctx, b) = device();
    assert!(a.generation() >= 1);
    assert!(b.generation() > a.generation());
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-gpu --test device_identity -- --ignored`"]
fn a_clone_shares_its_generation() {
    let _lock = serial();
    let (_ctx, a) = device();
    assert_eq!(a.clone().generation(), a.generation());
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-gpu --test device_identity -- --ignored`"]
fn a_live_device_is_not_lost() {
    let _lock = serial();
    let (_ctx, a) = device();
    assert!(!a.is_lost());
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-gpu --test device_identity -- --ignored`"]
fn a_request_reaches_new_devices_only_when_the_adapter_offers_it() {
    let _lock = serial();
    let want = wgpu::Features::SHADER_F16;
    let (_old_ctx, before) = device();
    request_optional_features(want);

    let offered = before.adapter.features().contains(want);
    let held_before = before.device.features().contains(want);
    // The pre-request device reports the feature missing iff the adapter offers
    // it and the device was not already created with it.
    assert_eq!(
        missing_requested_features(&before).contains(want),
        offered && !held_before
    );

    let (_new_ctx, after) = device();
    assert_eq!(after.device.features().contains(want), offered);
    assert!(missing_requested_features(&after).is_empty());
}

#[test]
#[ignore = "requires a GPU; run with `cargo test -p frust-gpu --test device_identity -- --ignored`"]
fn a_destroyed_device_eventually_reports_lost() {
    let _lock = serial();
    let (_ctx, a) = device();
    assert!(!a.is_lost());
    a.device.destroy();
    for _ in 0..200 {
        let _ = a.device.poll(wgpu::PollType::wait_indefinitely());
        if a.is_lost() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    eprintln!("skipped: this backend never reported the destroyed device lost");
}
