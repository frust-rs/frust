//! Android drive pipeline for `forgekit run` (spec §12.4): preflight →
//! gradle build → adb install → launch → pid-scoped logcat streaming.
//! Device selection (this module's top level) is decoupled from clap/stdin
//! so it's unit-testable without a terminal.

pub mod adb;
pub mod gradle;
pub mod preflight;
pub mod project;

use std::io::IsTerminal;

use crate::devices::{Device, Platform};

/// Outcome of matching discovered devices against `-d`/no-flag selection
/// (spec §12.4 step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelection {
    /// A single device to run on: either `-d` matched exactly one Android
    /// device, or exactly one Android device was discovered with no `-d`.
    Auto(Device),
    /// No Android device connected and no `-d` given: desktop fallback
    /// (`cargo run` passthrough).
    Desktop,
    /// Multiple Android devices and no `-d`: caller prompts (TTY) or
    /// lists-and-exits (non-TTY).
    Ambiguous(Vec<Device>),
    /// `-d` matched zero/multiple devices, or matched a non-Android device.
    Error(String),
}

/// Resolves device selection from all discovered devices and an optional
/// `-d` pattern (prefix match on id or name, case-insensitive).
pub fn select_device(devices: &[Device], pattern: Option<&str>) -> DeviceSelection {
    if let Some(pattern) = pattern {
        let matches: Vec<&Device> = devices
            .iter()
            .filter(|d| matches_pattern(d, pattern))
            .collect();
        return match matches.as_slice() {
            [] => DeviceSelection::Error(format!("no device matching `{pattern}`")),
            // Platform/kind-specific handling (Android drive pipeline, iOS
            // simulator drive pipeline, or a Phase 5 sentinel for physical
            // iOS devices) is the caller's job — `commands::run::run_on_device`
            // dispatches on `Device::platform`/`Device::kind`.
            [device] => DeviceSelection::Auto((*device).clone()),
            many => DeviceSelection::Error(format!(
                "ambiguous device id `{pattern}`; matches: {}",
                many.iter()
                    .map(|d| d.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }

    let android: Vec<Device> = devices
        .iter()
        .filter(|d| d.platform == Platform::Android)
        .cloned()
        .collect();
    match android.len() {
        0 => DeviceSelection::Desktop,
        1 => DeviceSelection::Auto(android.into_iter().next().expect("len checked")),
        _ => DeviceSelection::Ambiguous(android),
    }
}

fn matches_pattern(device: &Device, pattern: &str) -> bool {
    let pattern = pattern.to_lowercase();
    device.id.to_lowercase().starts_with(&pattern)
        || device.name.to_lowercase().starts_with(&pattern)
}

/// Parses a 1-based numbered-prompt answer (e.g. `"2"`) against `count`
/// choices, returning a 0-based index. Pure so the prompt's parsing is
/// unit-testable without stdin.
pub fn parse_prompt_selection(input: &str, count: usize) -> Result<usize, String> {
    let trimmed = input.trim();
    let choice: usize = trimmed
        .parse()
        .map_err(|_| format!("`{trimmed}` is not a number"))?;
    if choice == 0 || choice > count {
        return Err(format!("enter a number between 1 and {count}"));
    }
    Ok(choice - 1)
}

/// Whether stdout is attached to a terminal — decides numbered-prompt vs
/// list-and-exit for [`DeviceSelection::Ambiguous`].
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Kind;

    fn android(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Android,
            kind: Kind::Emulator,
        }
    }

    fn ios(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
        }
    }

    #[test]
    fn no_devices_and_no_pattern_falls_back_to_desktop() {
        assert_eq!(select_device(&[], None), DeviceSelection::Desktop);
    }

    #[test]
    fn only_ios_devices_and_no_pattern_falls_back_to_desktop() {
        let devices = vec![ios("sim1", "iPhone 15")];
        assert_eq!(select_device(&devices, None), DeviceSelection::Desktop);
    }

    #[test]
    fn exactly_one_android_device_auto_picked() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, None),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn multiple_android_devices_are_ambiguous() {
        let devices = vec![
            android("emulator-5554", "Pixel 7"),
            android("R58N90ABCDE", "Pixel 8"),
        ];
        assert_eq!(
            select_device(&devices, None),
            DeviceSelection::Ambiguous(devices.clone())
        );
    }

    #[test]
    fn dash_d_prefix_matches_id() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, Some("emulator")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn dash_d_prefix_matches_name_case_insensitively() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, Some("pixel")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn dash_d_no_match_errs() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        match select_device(&devices, Some("nope")) {
            DeviceSelection::Error(msg) => assert!(msg.contains("no device matching")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn dash_d_ambiguous_match_errs() {
        let devices = vec![
            android("emulator-5554", "Pixel 7"),
            android("emulator-5556", "Pixel 8"),
        ];
        match select_device(&devices, Some("emulator")) {
            DeviceSelection::Error(msg) => assert!(msg.contains("ambiguous")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn dash_d_matching_ios_device_is_auto_selected() {
        // `select_device` no longer special-cases iOS: platform/kind
        // dispatch (iOS simulator drive pipeline vs. a Phase 5 sentinel for
        // physical devices) is `commands::run::run_on_device`'s job.
        let devices = vec![ios("sim1", "iPhone 15")];
        assert_eq!(
            select_device(&devices, Some("sim1")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn parse_prompt_selection_accepts_in_range_number() {
        assert_eq!(parse_prompt_selection("2", 3), Ok(1));
        assert_eq!(parse_prompt_selection(" 1 \n", 3), Ok(0));
    }

    #[test]
    fn parse_prompt_selection_rejects_out_of_range() {
        assert!(parse_prompt_selection("0", 3).is_err());
        assert!(parse_prompt_selection("4", 3).is_err());
    }

    #[test]
    fn parse_prompt_selection_rejects_non_numeric() {
        assert!(parse_prompt_selection("abc", 3).is_err());
    }
}
