//! Pluggable doctor validators. Each validator is independent and returns
//! an actionable pass/partial/fail result; `frust doctor` prints
//! `[✓]`/`[!]`/`[✗]` per validator and exits 1 if any is `Fail`.

mod android_sdk;
mod cargo_ndk;
mod cargo_packager;
mod mobile_targets;
pub mod report;
mod rust_toolchain;
// `pub(crate)`: the browser pipeline's own preflight
// (`crate::web_build::preflight`) reads these modules' probe helpers so its
// rows and the flat doctor rows can never disagree about the same host — the
// severity each surface attaches to a probe result still differs, and stays
// with the caller.
pub(crate) mod wasm_bindgen_cli;
pub(crate) mod wasm_opt;
pub(crate) mod wasm_target;
mod xcode;

pub use android_sdk::AndroidSdkValidator;
pub use cargo_ndk::CargoNdkValidator;
pub use cargo_packager::CargoPackagerValidator;
pub use mobile_targets::MobileTargetsValidator;
pub use report::{Area, Component, ComponentStatus, DoctorReport, FixCommand, build_report};
pub use rust_toolchain::RustToolchainValidator;
pub use wasm_bindgen_cli::WasmBindgenCliValidator;
pub use wasm_opt::WasmOptValidator;
pub use wasm_target::WasmTargetValidator;
pub use xcode::XcodeValidator;

use crate::process::ProcessRunner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Partial,
    Fail,
}

/// A validator's result. `messages` must be actionable (e.g. the exact
/// command to run to fix a failure).
#[derive(Debug, Clone)]
pub struct Validation {
    pub status: Status,
    pub messages: Vec<String>,
}

/// Abstracts environment-variable lookups so validators are testable without
/// mutating the real (global, unsafe-to-mutate) process environment.
pub trait EnvLookup {
    fn get(&self, key: &str) -> Option<String>;
}

pub struct RealEnv;

impl EnvLookup for RealEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

pub struct DoctorCtx<'a> {
    pub runner: &'a dyn ProcessRunner,
    pub env: &'a dyn EnvLookup,
    pub is_macos: bool,
}

pub trait Validator {
    fn name(&self) -> &str;
    fn validate(&self, ctx: &DoctorCtx) -> Validation;
}

/// The v1 validator set, in report order, for a host known to be (or not be)
/// macOS. `XcodeValidator` is registered only when `is_macos` is `true`: the
/// structured report (`report::build_report`) already gates its whole iOS
/// area out off macOS, and the flat list must agree — otherwise a non-macOS
/// `frust doctor` run carries a permanent `[!] Xcode` row for a component
/// that can never exist on the host.
pub fn default_validators_for_host(is_macos: bool) -> Vec<Box<dyn Validator>> {
    let mut validators: Vec<Box<dyn Validator>> = vec![
        Box::new(RustToolchainValidator),
        Box::new(MobileTargetsValidator),
        Box::new(CargoNdkValidator),
        Box::new(AndroidSdkValidator),
    ];
    if is_macos {
        validators.push(Box::new(XcodeValidator));
    }
    validators.extend([
        Box::new(CargoPackagerValidator) as Box<dyn Validator>,
        Box::new(WasmTargetValidator),
        Box::new(WasmBindgenCliValidator),
        Box::new(WasmOptValidator),
    ]);
    validators
}

/// The v1 validator set for the current host. See
/// [`default_validators_for_host`] for the macOS-only `Xcode` gate, applied
/// here via `cfg!(target_os = "macos")`.
pub fn default_validators() -> Vec<Box<dyn Validator>> {
    default_validators_for_host(cfg!(target_os = "macos"))
}

/// Runs every validator against `ctx`, in order.
pub fn run_all(ctx: &DoctorCtx, validators: &[Box<dyn Validator>]) -> Vec<(String, Validation)> {
    validators
        .iter()
        .map(|v| (v.name().to_string(), v.validate(ctx)))
        .collect()
}

/// Test-only [`EnvLookup`] with an in-memory map, shared by validator unit tests.
#[cfg(test)]
pub(crate) struct FakeEnv(std::collections::HashMap<String, String>);

#[cfg(test)]
impl FakeEnv {
    pub(crate) fn new() -> Self {
        Self(std::collections::HashMap::new())
    }

    pub(crate) fn set(mut self, key: &str, value: &str) -> Self {
        self.0.insert(key.to_string(), value.to_string());
        self
    }
}

#[cfg(test)]
impl EnvLookup for FakeEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat validator list must never carry an `Xcode` row on a host
    /// that cannot have Xcode at all, and must always carry one on a macOS
    /// host — the same host-honest gate `report::build_report` already
    /// applies to the whole iOS area.
    #[test]
    fn xcode_validator_is_registered_only_on_macos() {
        let non_macos = default_validators_for_host(false);
        let non_macos_names: Vec<&str> = non_macos.iter().map(|v| v.name()).collect();
        assert!(
            !non_macos_names.contains(&"Xcode"),
            "non-macOS validator list must have no Xcode row: {non_macos_names:?}"
        );

        let macos = default_validators_for_host(true);
        let macos_names: Vec<&str> = macos.iter().map(|v| v.name()).collect();
        assert!(
            macos_names.contains(&"Xcode"),
            "macOS validator list must have an Xcode row: {macos_names:?}"
        );
    }
}
