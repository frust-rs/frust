//! Desktop entry point of the hot-patch spike.
//!
//! A spike-only deviation from the template's "never hand-edit `main.rs`" rule: the desktop arm first
//! connects to dx's devserver (`dioxus_devtools::connect_subsecond`, a no-op when the binary was not
//! launched by `dx serve`), so patches dx builds for the `app` package reach this process. The
//! template's `wasm32` arm is absent on purpose: it exists only because a one-package app's `[lib]`
//! and `[[bin]]` collide on one `.wasm` output name, and here the lib is another package.

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn main() {
    // `dioxus-devtools` is a desktop-only dependency; iOS shares this arm only to reach the
    // `__frust_main` stub `frust::app!` emits there.
    #[cfg(not(target_os = "ios"))]
    dioxus_devtools::connect_subsecond();
    hotpatch_spike_app::__frust_main();
}

#[cfg(target_os = "android")]
fn main() {}
