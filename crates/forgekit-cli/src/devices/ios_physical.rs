use super::{Device, DeviceDiscovery, DiscoveryResult, Kind, Platform};
use crate::process::ProcessRunner;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

pub struct IosPhysicalDiscovery;

impl DeviceDiscovery for IosPhysicalDiscovery {
    fn discover(&self, runner: &dyn ProcessRunner) -> Result<DiscoveryResult> {
        if !cfg!(target_os = "macos") {
            return Ok(DiscoveryResult {
                devices: Vec::new(),
                notes: vec!["iOS physical device discovery skipped (not macOS)".to_string()],
            });
        }

        let tmp_path = tmp_json_path();
        let tmp_str = tmp_path.to_string_lossy().to_string();

        let run_result = runner.run(
            "xcrun",
            &["devicectl", "list", "devices", "--json-output", &tmp_str],
        );

        match run_result {
            Ok(out) if out.success => {}
            Ok(out) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Ok(DiscoveryResult {
                    devices: Vec::new(),
                    notes: vec![format!("`xcrun devicectl` failed: {}", out.stderr.trim())],
                });
            }
            Err(_) => {
                // devicectl is absent pre-Xcode-15 — tolerate silently in the
                // listing, note only under `-v`.
                return Ok(DiscoveryResult {
                    devices: Vec::new(),
                    notes: vec!["xcrun devicectl not found (requires Xcode 15+); iOS physical device discovery skipped".to_string()],
                });
            }
        };

        let content = std::fs::read_to_string(&tmp_path)
            .with_context(|| format!("failed to read devicectl output at {}", tmp_path.display()));
        let _ = std::fs::remove_file(&tmp_path);

        parse_devicectl_output(&content?)
    }
}

fn tmp_json_path() -> PathBuf {
    std::env::temp_dir().join(format!("forgekit-devicectl-{}.json", std::process::id()))
}

/// `xcrun devicectl list devices --json-output <file>` JSON structure:
/// `{"result": {"devices": [{identifier, deviceProperties: {name, osVersionNumber}, connectionProperties: {tunnelState, pairingState}}]}}`.
#[derive(Debug, Deserialize)]
struct DevicectlOutput {
    result: DevicectlResult,
}

#[derive(Debug, Deserialize)]
struct DevicectlResult {
    devices: Vec<DevicectlDevice>,
}

#[derive(Debug, Deserialize)]
struct DevicectlDevice {
    identifier: String,
    #[serde(rename = "deviceProperties")]
    device_properties: DevicectlDeviceProperties,
    #[serde(rename = "connectionProperties")]
    connection_properties: DevicectlConnectionProperties,
}

#[derive(Debug, Deserialize)]
struct DevicectlDeviceProperties {
    name: String,
    /// The device's OS version (e.g. `"17.5.1"`), gating `ios_run::run_physical`'s
    /// devicectl-requires-iOS-17+ check (task 67). Absent on older `devicectl`
    /// output shapes, so this stays optional rather than a hard parse failure.
    #[serde(rename = "osVersionNumber", default)]
    os_version_number: Option<String>,
}

/// Both connection fields are `Option` with lenient defaults: devicectl's
/// JSON schema is undocumented and field presence varies across Xcode
/// releases (the same tolerance `osVersionNumber` above already has).
#[derive(Debug, Deserialize)]
struct DevicectlConnectionProperties {
    #[serde(rename = "tunnelState", default)]
    tunnel_state: Option<String>,
    #[serde(rename = "pairingState", default)]
    pairing_state: Option<String>,
}

fn parse_devicectl_output(content: &str) -> Result<DiscoveryResult> {
    let parsed: DevicectlOutput = serde_json::from_str(content)?;
    let mut devices = Vec::new();
    let mut notes = Vec::new();

    for device in parsed.result.devices {
        let conn = &device.connection_properties;
        // Include paired (or pairing-unknown) devices whose tunnel is
        // "connected" OR "disconnected": CoreDevice establishes the tunnel
        // LAZILY on the first install/launch, so a paired, USB-plugged,
        // idle iPhone normally lists as "disconnected" and is fully
        // targetable (verified on-device — see
        // workflow/plans/bugs/ios-device-discovery-tunnelstate/BUG.md).
        // Only "unavailable" (known to CoreDevice but not currently
        // reachable) and explicitly non-paired devices are skipped.
        if let Some(pairing) = conn.pairing_state.as_deref()
            && pairing != "paired"
        {
            notes.push(format!(
                "skipping {} ({}): pairingState {pairing} — pair and trust this computer",
                device.device_properties.name, device.identifier,
            ));
            continue;
        }
        if conn.tunnel_state.as_deref() == Some("unavailable") {
            notes.push(format!(
                "skipping {} ({}): tunnelState unavailable (device not currently reachable)",
                device.device_properties.name, device.identifier,
            ));
            continue;
        }
        devices.push(Device {
            id: device.identifier,
            name: device.device_properties.name,
            platform: Platform::Ios,
            kind: Kind::PhysicalDevice,
            os_version: device.device_properties.os_version_number,
            connection_state: conn.tunnel_state.clone(),
        });
    }

    Ok(DiscoveryResult { devices, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "result": {
            "devices": [
                {
                    "identifier": "00008110-000A2D3A3C68801E",
                    "deviceProperties": { "name": "Ed's iPhone" },
                    "connectionProperties": { "tunnelState": "connected", "pairingState": "paired" }
                },
                {
                    "identifier": "00008120-001A2D3A3C68802F",
                    "deviceProperties": { "name": "Idle iPhone" },
                    "connectionProperties": { "tunnelState": "disconnected", "pairingState": "paired" }
                },
                {
                    "identifier": "00008130-002A2D3A3C68803A",
                    "deviceProperties": { "name": "Wifi iPad" },
                    "connectionProperties": { "tunnelState": "unavailable", "pairingState": "paired" }
                },
                {
                    "identifier": "00008140-003A2D3A3C68804B",
                    "deviceProperties": { "name": "Strange iPhone" },
                    "connectionProperties": { "tunnelState": "disconnected", "pairingState": "unpaired" }
                }
            ]
        }
    }"#;

    const FIXTURE_WITH_VERSION: &str = r#"{
        "result": {
            "devices": [
                {
                    "identifier": "00008110-000A2D3A3C68801E",
                    "deviceProperties": { "name": "Ed's iPhone", "osVersionNumber": "17.5.1" },
                    "connectionProperties": { "tunnelState": "connected" }
                }
            ]
        }
    }"#;

    #[test]
    fn includes_paired_disconnected_excludes_unavailable_and_unpaired() {
        let result = parse_devicectl_output(FIXTURE).unwrap();
        // "connected" AND paired-"disconnected" are both targetable — the
        // tunnel is established lazily by install/launch (the bug this
        // module's old `== "connected"` filter caused: a paired USB iPhone
        // was invisible to `forgekit devices` and unselectable by `-d`).
        assert_eq!(result.devices.len(), 2);
        assert_eq!(result.devices[0].name, "Ed's iPhone");
        assert_eq!(result.devices[0].kind, Kind::PhysicalDevice);
        assert_eq!(result.devices[0].platform, Platform::Ios);
        assert_eq!(result.devices[0].os_version, None);
        assert_eq!(
            result.devices[0].connection_state.as_deref(),
            Some("connected")
        );
        assert_eq!(result.devices[1].name, "Idle iPhone");
        assert_eq!(
            result.devices[1].connection_state.as_deref(),
            Some("disconnected")
        );
        // "unavailable" (unreachable) and non-paired devices stay out, each
        // with a diagnosable note.
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("Wifi iPad") && n.contains("unavailable"))
        );
        assert!(
            result
                .notes
                .iter()
                .any(|n| n.contains("Strange iPhone") && n.contains("unpaired"))
        );
        assert_eq!(result.notes.len(), 2);
    }

    #[test]
    fn missing_connection_fields_are_tolerated() {
        // Older devicectl output shapes omit fields — a device with no
        // pairingState/tunnelState still lists (lenient like osVersionNumber).
        let json = r#"{
            "result": {
                "devices": [
                    {
                        "identifier": "00008150-004A2D3A3C68805C",
                        "deviceProperties": { "name": "Bare iPhone" },
                        "connectionProperties": {}
                    }
                ]
            }
        }"#;
        let result = parse_devicectl_output(json).unwrap();
        assert_eq!(result.devices.len(), 1);
        assert_eq!(result.devices[0].name, "Bare iPhone");
        assert_eq!(result.devices[0].connection_state, None);
        assert!(result.notes.is_empty());
    }

    #[test]
    fn parses_os_version_number_when_present() {
        let result = parse_devicectl_output(FIXTURE_WITH_VERSION).unwrap();
        assert_eq!(result.devices.len(), 1);
        assert_eq!(result.devices[0].os_version.as_deref(), Some("17.5.1"));
    }

    #[test]
    fn empty_devices_yields_no_devices() {
        let result = parse_devicectl_output(r#"{"result": {"devices": []}}"#).unwrap();
        assert!(result.devices.is_empty());
        assert!(result.notes.is_empty());
    }

    #[test]
    fn invalid_json_errs() {
        assert!(parse_devicectl_output("not json").is_err());
    }
}
