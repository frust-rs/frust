//! Device discovery (spec §12.4). Each discoverer is independent and must
//! not fail `forgekit devices` just because its underlying tool is absent
//! (e.g. no Android SDK on a Mac) — it returns an empty list plus a `-v` note.

mod android;
mod ios_physical;
mod ios_simulator;

pub use android::AndroidDeviceDiscovery;
pub use ios_physical::IosPhysicalDiscovery;
pub use ios_simulator::IosSimulatorDiscovery;

use crate::process::ProcessRunner;
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    PhysicalDevice,
    Emulator,
    Simulator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub kind: Kind,
}

/// Result of a single discoverer's run: devices found, plus human-readable
/// notes (missing tool, skipped unauthorized device, …) shown only with `-v`.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryResult {
    pub devices: Vec<Device>,
    pub notes: Vec<String>,
}

pub trait DeviceDiscovery {
    fn discover(&self, runner: &dyn ProcessRunner) -> Result<DiscoveryResult>;
}

/// The v1 discoverer set (spec §12.4), in report order.
pub fn default_discoverers() -> Vec<Box<dyn DeviceDiscovery>> {
    vec![
        Box::new(AndroidDeviceDiscovery),
        Box::new(IosSimulatorDiscovery),
        Box::new(IosPhysicalDiscovery),
    ]
}

/// Runs every discoverer against `runner`, aggregating devices and notes.
/// A discoverer erroring out (rather than returning `Ok(DiscoveryResult)`)
/// is downgraded to a note — a single flaky discoverer must not blank the
/// whole `forgekit devices` listing.
pub fn discover_all(
    runner: &dyn ProcessRunner,
    discoverers: &[Box<dyn DeviceDiscovery>],
) -> (Vec<Device>, Vec<String>) {
    let mut devices = Vec::new();
    let mut notes = Vec::new();
    for discoverer in discoverers {
        match discoverer.discover(runner) {
            Ok(mut result) => {
                devices.append(&mut result.devices);
                notes.append(&mut result.notes);
            }
            Err(err) => notes.push(format!("device discovery error: {err:#}")),
        }
    }
    (devices, notes)
}
