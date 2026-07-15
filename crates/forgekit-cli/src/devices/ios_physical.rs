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
/// `{"result": {"devices": [{identifier, deviceProperties: {name}, connectionProperties: {tunnelState}}]}}`.
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
}

#[derive(Debug, Deserialize)]
struct DevicectlConnectionProperties {
    #[serde(rename = "tunnelState")]
    tunnel_state: String,
}

fn parse_devicectl_output(content: &str) -> Result<DiscoveryResult> {
    let parsed: DevicectlOutput = serde_json::from_str(content)?;
    let mut devices = Vec::new();
    let mut notes = Vec::new();

    for device in parsed.result.devices {
        if device.connection_properties.tunnel_state != "connected" {
            notes.push(format!(
                "skipping {} ({}): {}",
                device.device_properties.name,
                device.identifier,
                device.connection_properties.tunnel_state
            ));
            continue;
        }
        devices.push(Device {
            id: device.identifier,
            name: device.device_properties.name,
            platform: Platform::Ios,
            kind: Kind::PhysicalDevice,
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
                    "connectionProperties": { "tunnelState": "connected" }
                },
                {
                    "identifier": "00008120-001A2D3A3C68802F",
                    "deviceProperties": { "name": "Old iPad" },
                    "connectionProperties": { "tunnelState": "disconnected" }
                }
            ]
        }
    }"#;

    #[test]
    fn parses_only_connected_devices() {
        let result = parse_devicectl_output(FIXTURE).unwrap();
        assert_eq!(result.devices.len(), 1);
        assert_eq!(result.devices[0].name, "Ed's iPhone");
        assert_eq!(result.devices[0].kind, Kind::PhysicalDevice);
        assert_eq!(result.devices[0].platform, Platform::Ios);
        assert!(result.notes.iter().any(|n| n.contains("Old iPad")));
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
