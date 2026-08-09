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
pub mod build_info;
pub mod cargo_manifest;
pub mod devices;
pub mod devtools_client;
pub mod doctor;
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
