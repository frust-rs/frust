use super::{Device, DeviceDiscovery, DiscoveryResult, Kind, Platform};
use crate::process::ProcessRunner;
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

pub struct IosSimulatorDiscovery;

impl DeviceDiscovery for IosSimulatorDiscovery {
    fn discover(&self, runner: &dyn ProcessRunner) -> Result<DiscoveryResult> {
        if !cfg!(target_os = "macos") {
            return Ok(DiscoveryResult {
                devices: Vec::new(),
                notes: vec!["iOS simulator discovery skipped (not macOS)".to_string()],
            });
        }

        let out = match runner.run("xcrun", &["simctl", "list", "devices", "--json"]) {
            Ok(out) if out.success => out,
            _ => {
                return Ok(DiscoveryResult {
                    devices: Vec::new(),
                    notes: vec![
                        "xcrun simctl not found; iOS simulator discovery skipped".to_string(),
                    ],
                });
            }
        };

        parse_simctl_list(&out.stdout)
    }
}

/// `xcrun simctl list devices --json` JSON structure:
/// `{"devices": {"<runtime>": [{udid, name, state, isAvailable}]}}`.
#[derive(Debug, Deserialize)]
struct SimctlList {
    devices: HashMap<String, Vec<SimctlDevice>>,
}

#[derive(Debug, Deserialize)]
struct SimctlDevice {
    udid: String,
    name: String,
    state: String,
}

fn parse_simctl_list(stdout: &str) -> Result<DiscoveryResult> {
    let parsed: SimctlList = serde_json::from_str(stdout)?;
    let mut devices = Vec::new();
    for list in parsed.devices.values() {
        for device in list {
            if device.state == "Booted" {
                devices.push(Device {
                    id: device.udid.clone(),
                    name: device.name.clone(),
                    platform: Platform::Ios,
                    kind: Kind::Simulator,
                    os_version: None,
                    connection_state: None,
                });
            }
        }
    }
    Ok(DiscoveryResult {
        devices,
        notes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
                {
                    "udid": "AAAAAAAA-1111-2222-3333-444444444444",
                    "name": "iPhone 15",
                    "state": "Booted",
                    "isAvailable": true
                },
                {
                    "udid": "BBBBBBBB-1111-2222-3333-444444444444",
                    "name": "iPhone 15 Pro",
                    "state": "Shutdown",
                    "isAvailable": true
                }
            ]
        }
    }"#;

    #[test]
    fn parses_only_booted_simulators() {
        let result = parse_simctl_list(FIXTURE).unwrap();
        assert_eq!(result.devices.len(), 1);
        assert_eq!(result.devices[0].name, "iPhone 15");
        assert_eq!(result.devices[0].kind, Kind::Simulator);
        assert_eq!(result.devices[0].platform, Platform::Ios);
    }

    #[test]
    fn empty_devices_map_yields_no_devices() {
        let result = parse_simctl_list(r#"{"devices": {}}"#).unwrap();
        assert!(result.devices.is_empty());
    }

    #[test]
    fn invalid_json_errs() {
        assert!(parse_simctl_list("not json").is_err());
    }
}
