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
use std::collections::HashSet;

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
    let devices = without_simulator_twins(devices, &mut notes);
    (devices, notes)
}

/// `devices` without the iOS *physical* entries that share a listed
/// simulator's udid. Xcode 27's `devicectl` reports a booted simulator as a
/// paired, connected device under the identifier `simctl` lists, so every
/// `-d` naming it would be ambiguous. A simulator udid never names a
/// physical device, so the simulator entry is the one kept; each dropped
/// twin leaves a `-v` note.
fn without_simulator_twins(devices: Vec<Device>, notes: &mut Vec<String>) -> Vec<Device> {
    let simulators: HashSet<String> = devices
        .iter()
        .filter(|d| d.platform == Platform::Ios && d.kind == Kind::Simulator)
        .map(|d| d.id.clone())
        .collect();
    devices
        .into_iter()
        .filter(|d| {
            let twin = d.platform == Platform::Ios
                && d.kind == Kind::PhysicalDevice
                && simulators.contains(&d.id);
            if twin {
                notes.push(format!(
                    "devicectl also lists simulator {} ({}); kept the simulator entry",
                    d.name, d.id,
                ));
            }
            !twin
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulator udid in the recorded listings below (anonymised).
    const SIM_UDID: &str = "5A3E2C1D-7F4B-4E6A-9C8D-0B1A2F3E4D5C";
    /// A paired physical iPhone's CoreDevice identifier (anonymised).
    const PHONE_ID: &str = "C0FFEE00-1234-5678-9ABC-DEF012345678";

    /// `xcrun devicectl list devices --json-output` on Xcode 27.0 with the
    /// simulator booted, trimmed to the fields that matter: the simulator is
    /// listed as a paired, connected device under its `simctl` udid.
    const RECORDED_DEVICECTL_TWIN: &str = r#"{
        "info": { "commandType": "devicectl.list.devices", "jsonVersion": 5, "outcome": "success", "version": "642.16" },
        "result": {
            "devices": [
                {
                    "identifier": "C0FFEE00-1234-5678-9ABC-DEF012345678",
                    "deviceProperties": { "name": "Test iPhone", "osVersionNumber": "26.7", "bootState": "booted" },
                    "connectionProperties": { "pairingState": "paired", "tunnelState": "disconnected", "transportType": "localNetwork" },
                    "hardwareProperties": { "reality": "physical", "platform": "iOS", "deviceType": "iPhone" }
                },
                {
                    "identifier": "5A3E2C1D-7F4B-4E6A-9C8D-0B1A2F3E4D5C",
                    "deviceProperties": { "name": "iPhone 18 Pro", "osVersionNumber": "27.0", "bootState": "booted" },
                    "connectionProperties": { "pairingState": "paired", "tunnelState": "connected", "transportType": "sameMachine" },
                    "hardwareProperties": { "reality": "simulated", "platform": "iOS", "deviceType": "iPhone" }
                }
            ]
        }
    }"#;

    /// The same listing without the simulator entry (pre-Xcode-27 shape).
    const RECORDED_DEVICECTL_NO_TWIN: &str = r#"{
        "info": { "commandType": "devicectl.list.devices", "jsonVersion": 5, "outcome": "success", "version": "642.16" },
        "result": {
            "devices": [
                {
                    "identifier": "C0FFEE00-1234-5678-9ABC-DEF012345678",
                    "deviceProperties": { "name": "Test iPhone", "osVersionNumber": "26.7", "bootState": "booted" },
                    "connectionProperties": { "pairingState": "paired", "tunnelState": "disconnected", "transportType": "localNetwork" },
                    "hardwareProperties": { "reality": "physical", "platform": "iOS", "deviceType": "iPhone" }
                }
            ]
        }
    }"#;

    /// `xcrun simctl list devices --json` on the same host, trimmed.
    #[cfg(target_os = "macos")]
    const RECORDED_SIMCTL: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0": [
                {
                    "udid": "5A3E2C1D-7F4B-4E6A-9C8D-0B1A2F3E4D5C",
                    "isAvailable": true,
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-18-Pro",
                    "state": "Booted",
                    "name": "iPhone 18 Pro"
                }
            ]
        }
    }"#;

    /// Hands back a fixed result, standing in for one discoverer.
    struct Canned(DiscoveryResult);

    impl DeviceDiscovery for Canned {
        fn discover(&self, _runner: &dyn ProcessRunner) -> Result<DiscoveryResult> {
            Ok(self.0.clone())
        }
    }

    /// The booted simulator as `IosSimulatorDiscovery` reports it.
    fn booted_simulator(udid: &str) -> Device {
        Device {
            id: udid.to_string(),
            name: "iPhone 18 Pro".to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    fn discover_canned(simulator: Device, devicectl: &str) -> (Vec<Device>, Vec<String>) {
        let discoverers: Vec<Box<dyn DeviceDiscovery>> = vec![
            Box::new(Canned(DiscoveryResult {
                devices: vec![simulator],
                notes: Vec::new(),
            })),
            Box::new(Canned(
                ios_physical::parse_devicectl_output(devicectl).unwrap(),
            )),
        ];
        discover_all(&crate::process::FakeProcessRunner::new(), &discoverers)
    }

    fn ids_and_kinds(devices: &[Device]) -> Vec<(&str, Kind)> {
        devices.iter().map(|d| (d.id.as_str(), d.kind)).collect()
    }

    #[test]
    fn a_booted_simulator_devicectl_also_lists_is_reported_once() {
        let (devices, notes) = discover_canned(booted_simulator(SIM_UDID), RECORDED_DEVICECTL_TWIN);
        assert_eq!(
            ids_and_kinds(&devices),
            vec![
                (SIM_UDID, Kind::Simulator),
                (PHONE_ID, Kind::PhysicalDevice),
            ],
        );
        assert!(
            notes
                .iter()
                .any(|n| n.contains(SIM_UDID) && n.contains("kept the simulator"))
        );
    }

    #[test]
    fn a_physical_device_with_a_distinct_udid_is_still_listed() {
        let (devices, notes) =
            discover_canned(booted_simulator(SIM_UDID), RECORDED_DEVICECTL_NO_TWIN);
        assert_eq!(
            ids_and_kinds(&devices),
            vec![
                (SIM_UDID, Kind::Simulator),
                (PHONE_ID, Kind::PhysicalDevice),
            ],
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn devicectl_entries_survive_when_no_listed_simulator_shares_their_id() {
        // The twin listing with a different simulator booted: nothing to
        // drop, so devicectl's simulator entry stays as reported.
        let other = "0D1C2B3A-4F5E-4A6B-8C7D-9E8F7A6B5C4D";
        let (devices, notes) = discover_canned(booted_simulator(other), RECORDED_DEVICECTL_TWIN);
        assert_eq!(devices.len(), 3);
        assert!(notes.is_empty());
    }

    /// The real discoverers fed the recorded listings: the merged
    /// `discover_all` output holds the simulator once, as a simulator.
    #[cfg(target_os = "macos")]
    #[test]
    fn recorded_xcode_27_listings_merge_to_one_simulator_entry() {
        use crate::process::{FakeProcessRunner, Output};
        let ok = Output {
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        };
        let simctl = Output {
            stdout: RECORDED_SIMCTL.to_string(),
            ..ok.clone()
        };
        let discoverers: Vec<Box<dyn DeviceDiscovery>> = vec![
            Box::new(IosSimulatorDiscovery),
            Box::new(IosPhysicalDiscovery),
        ];
        // One test drives both listings in turn: `IosPhysicalDiscovery`'s
        // temp file is per-process, so parallel tests would share it.
        for (devicectl, simulator_notes) in [
            (RECORDED_DEVICECTL_TWIN, 1),
            (RECORDED_DEVICECTL_NO_TWIN, 0),
        ] {
            let runner = FakeProcessRunner::new()
                .with("xcrun simctl list devices --json", simctl.clone())
                .with_file(
                    "xcrun devicectl list devices --json-output",
                    ok.clone(),
                    devicectl,
                );
            let (devices, notes) = discover_all(&runner, &discoverers);
            assert_eq!(
                ids_and_kinds(&devices),
                vec![
                    (SIM_UDID, Kind::Simulator),
                    (PHONE_ID, Kind::PhysicalDevice),
                ],
            );
            assert_eq!(notes.len(), simulator_notes, "{notes:?}");
        }
    }
}
