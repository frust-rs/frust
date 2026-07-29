//! Device discovery. Each discoverer is independent and must not fail
//! `frust devices` just because its underlying tool is absent (e.g. no
//! Android SDK on a Mac) — it returns an empty list plus a `-v` note.

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
    /// The device's OS version, if trivially available from its discoverer
    /// (e.g. `devicectl`'s `osVersionNumber` for a physical iOS device,
    /// parsed by `ios_run::run_physical`'s iOS-17+ gate). `None` when the
    /// discoverer doesn't surface one (Android, iOS Simulator).
    pub os_version: Option<String>,
    /// The discoverer's raw connection-state string, surfaced in the
    /// `frust devices` listing (e.g. devicectl's `tunnelState`:
    /// "connected"/"disconnected" — the latter is a paired, targetable
    /// device whose CoreDevice tunnel comes up lazily on install/launch).
    /// `None` where the discoverer has no such notion (Android, Simulator).
    pub connection_state: Option<String>,
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

/// The v1 discoverer set, in report order.
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
/// whole `frust devices` listing.
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
