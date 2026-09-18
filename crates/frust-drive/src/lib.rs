//! `frust-drive`: the shared drive logic behind the `frust` CLI — project
//! scaffolding, environment doctor, device discovery, and the
//! `run`/`build`/`clean` drive pipelines for Android and iOS.
//!
//! Extracted from `frust-cli` so a second front-end (the `frust-tui`
//! terminal UI) can consume the same pipelines without a package cycle
//! (`frust-cli` → `frust-tui` → `frust-drive`, acyclic). This crate holds
//! no clap surface — the `#[derive(clap::Args)]` layer stays in `frust-cli`
//! and converts into [`build_info::BuildArgs`] at the command-handler
//! boundary — and, like `frust-cli`, depends on none of the framework
//! crates (`docs/ARCHITECTURE.md`).

pub mod android_build;
pub mod android_id;
pub mod android_run;
/// Single source of truth for where a Frust app's build output lives under
/// `build/`, and the directories [`clean`] removes ([`build_dirs::CLEAN_DIRS`]
/// plus the pre-migration [`build_dirs::LEGACY_CLEAN_DIRS`]).
pub mod build_dirs;
pub mod build_info;
pub mod cargo_manifest;
/// Print-free `frust clean` core: `cargo clean` via the injected
/// [`process::ProcessRunner`], then [`build_dirs::CLEAN_DIRS`]/
/// [`build_dirs::LEGACY_CLEAN_DIRS`] removal, reporting through an `on_line`
/// sink — the shared implementation `frust-cli`'s `clean` command and
/// `frust-tui`'s clean session both call into.
pub mod clean;
/// Desktop bundle assembly (`cargo build` + the per-OS `.app`/dist-dir/bundle
/// layouts, icons, optional macOS codesign) — host-locked per target.
pub mod desktop_build;
/// The desktop `cargo run` launch plan shared by any front-end previewing a
/// Frust project on the host (`frust-tui`'s own desktop session builder is
/// not converged onto this module yet — see the module doc).
pub mod desktop_run;
pub mod devices;
pub mod devtools_client;
pub mod doctor;
pub mod icons;
/// Process-wide termination handling (the single SIGINT/SIGTERM/SIGHUP owner
/// and the secret-file scrub it runs). Internal: it is machinery the pipelines
/// arm, not a surface a front-end drives.
pub(crate) mod interrupt;
pub mod ios_build;
pub mod ios_id;
pub mod ios_run;
pub mod manifest;
/// Streamable system-metrics collectors (per-process CPU/RSS/thermal,
/// coarse network counters), sampled tool-side via `/proc`+`/sys` on desktop
/// Linux or `adb` on Android — distinct from `devtools_client`'s in-app wire
/// protocol, this reads OS-level process/host stats a running app never
/// reports itself.
pub mod metrics;
pub mod plugin;
pub mod process;
pub mod scaffold;
/// Browser build assembly (`cargo build --target wasm32-unknown-unknown` +
/// `wasm-bindgen` + an optional `wasm-opt` pass into a servable artifact
/// directory), the static development server that hands that directory to a
/// browser, and the host-toolchain preflight both depend on. The web tier's
/// counterpart to [`desktop_build`]; unlike it, host-independent — a browser
/// artifact is the same on every OS.
pub mod web_build;
