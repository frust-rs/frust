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

/// The v1 validator set, in report order.
pub fn default_validators() -> Vec<Box<dyn Validator>> {
    vec![
        Box::new(RustToolchainValidator),
        Box::new(MobileTargetsValidator),
        Box::new(CargoNdkValidator),
        Box::new(AndroidSdkValidator),
        Box::new(XcodeValidator),
        Box::new(CargoPackagerValidator),
        Box::new(WasmTargetValidator),
        Box::new(WasmBindgenCliValidator),
        Box::new(WasmOptValidator),
    ]
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
