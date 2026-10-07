//! Desktop entry point of the hot-patch spike.
//!
//! A spike-only deviation from the template's "never hand-edit `main.rs`" rule: the desktop arm first
//! connects to dx's devserver (`dioxus_devtools::connect`, a no-op when the binary was not launched
//! by `dx serve`) and applies each patch dx builds for the `app` package through frust-hotpatch. The
//! template's `wasm32` arm is absent on purpose: it exists only because a one-package app's `[lib]`
//! and `[[bin]]` collide on one `.wasm` output name, and here the lib is another package.

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn main() {
    // `dioxus-devtools` is a desktop-only dependency; iOS shares this arm only to reach the
    // `__frust_main` stub `frust::app!` emits there. The devserver connection (and the `apply_patch`
    // it performs, which loads and jumps into code the devserver sends) must stay debug-only: a
    // release build never connects. frust-hotpatch itself consults its jump table only under
    // `debug_assertions`, so gating the connection the same way keeps both halves in step.
    #[cfg(all(debug_assertions, not(target_os = "ios")))]
    dioxus_devtools::connect(apply_hot_patch);
    hotpatch_spike_app::__frust_main();
}

/// Applies a devserver hot-patch addressed to this process through frust-hotpatch, the same filter
/// `dioxus_devtools::connect_subsecond` applies. dx sends a `subsecond_types::JumpTable`; the
/// frust-hotpatch table has the same serde shape, so a JSON round-trip converts it.
#[cfg(all(
    debug_assertions,
    not(any(target_os = "android", target_os = "ios", target_arch = "wasm32"))
))]
fn apply_hot_patch(msg: dioxus_devtools::DevserverMsg) {
    let dioxus_devtools::DevserverMsg::HotReload(msg) = msg else {
        return;
    };
    let Some(table) = msg.jump_table else {
        return;
    };
    if msg.for_pid != Some(std::process::id()) {
        return;
    }
    let table: frust_hotpatch::JumpTable =
        match serde_json::to_value(table).and_then(serde_json::from_value) {
            Ok(table) => table,
            Err(err) => {
                log::error!("frust-hotpatch: jump table conversion failed: {err}");
                return;
            }
        };
    // SAFETY: the table was built by the dx devserver that launched this exact binary and is
    // addressed to this process id, which is `apply_patch`'s contract.
    if let Err(err) = unsafe { frust_hotpatch::apply_patch(table) } {
        log::error!("frust-hotpatch: apply_patch failed: {err}");
    }
}

#[cfg(target_os = "android")]
fn main() {}
