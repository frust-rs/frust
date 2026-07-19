use super::{Device, DeviceDiscovery, DiscoveryResult, Kind, Platform};
use crate::process::ProcessRunner;
use anyhow::Result;

pub struct AndroidDeviceDiscovery;

impl DeviceDiscovery for AndroidDeviceDiscovery {
    fn discover(&self, runner: &dyn ProcessRunner) -> Result<DiscoveryResult> {
        let out = match runner.run("adb", &["devices", "-l"]) {
            Ok(out) if out.success => out,
            Ok(out) => {
                return Ok(DiscoveryResult {
                    devices: Vec::new(),
                    notes: vec![format!("`adb devices -l` failed: {}", out.stderr.trim())],
                });
            }
            Err(_) => {
                return Ok(DiscoveryResult {
                    devices: Vec::new(),
                    notes: vec!["adb not found; Android device discovery skipped".to_string()],
                });
            }
        };
        Ok(parse_adb_devices(&out.stdout))
    }
}

/// Parses `adb devices -l` output, e.g.:
/// ```text
/// List of devices attached
/// emulator-5554  device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emulator64_arm64 transport_id:1
/// R58N90ABCDE    device usb:1-1 product:otter model:Pixel_7 device:otter transport_id:2
/// 1234567890     unauthorized usb:1-1 transport_id:3
/// ```
fn parse_adb_devices(stdout: &str) -> DiscoveryResult {
    let mut devices = Vec::new();
    let mut notes = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("List of devices attached") {
            continue;
        }

        let mut fields = line.split_whitespace();
        let Some(id) = fields.next() else { continue };
        let Some(state) = fields.next() else { continue };

        if state == "unauthorized" || state == "offline" {
            notes.push(format!("skipping {id}: {state}"));
            continue;
        }
        if state != "device" {
            notes.push(format!("skipping {id}: unrecognized state '{state}'"));
            continue;
        }

        let mut model: Option<&str> = None;
        let mut device_field: Option<&str> = None;
        for field in fields {
            if let Some(v) = field.strip_prefix("model:") {
                model = Some(v);
            } else if let Some(v) = field.strip_prefix("device:") {
                device_field = Some(v);
            }
        }
        let name = model.or(device_field).unwrap_or(id).replace('_', " ");
        let kind = if id.starts_with("emulator-") {
            Kind::Emulator
        } else {
            Kind::PhysicalDevice
        };
        devices.push(Device {
            id: id.to_string(),
            name,
            platform: Platform::Android,
            kind,
            os_version: None,
            connection_state: None,
        });
    }

    DiscoveryResult { devices, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "List of devices attached\n\
emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emulator64_arm64 transport_id:1\n\
R58N90ABCDE             device usb:1-1 product:otter model:Pixel_7 device:otter transport_id:2\n\
1234567890              unauthorized usb:1-1 transport_id:3\n\
\n";

    #[test]
    fn parses_fixture_output() {
        let result = parse_adb_devices(FIXTURE);
        assert_eq!(result.devices.len(), 2);

        let emulator = &result.devices[0];
        assert_eq!(emulator.id, "emulator-5554");
        assert_eq!(emulator.name, "sdk gphone64 arm64");
        assert_eq!(emulator.kind, Kind::Emulator);
        assert_eq!(emulator.platform, Platform::Android);

        let physical = &result.devices[1];
        assert_eq!(physical.id, "R58N90ABCDE");
        assert_eq!(physical.name, "Pixel 7");
        assert_eq!(physical.kind, Kind::PhysicalDevice);

        assert!(result.notes.iter().any(|n| n.contains("1234567890")));
    }

    #[test]
    fn empty_list_yields_no_devices() {
        let result = parse_adb_devices("List of devices attached\n\n");
        assert!(result.devices.is_empty());
        assert!(result.notes.is_empty());
    }
}
