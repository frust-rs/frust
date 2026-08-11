//! Launch-argument resolution: what a `launch` request's three optional
//! fields mean once they reach the backend.
//!
//! The device rules mirror `frust-mcp`'s `run_app` tool
//! (`crates/frust-mcp/src/tools/session.rs`) deliberately — an agent and an
//! editor naming the same device must land on the same target. Those helpers
//! are private to that crate, so the behaviour is mirrored here rather than
//! imported; the matching order (exact id, then unique case-insensitive
//! substring of id or name) and the physical-iOS refusal are the parts that
//! must not drift.

use std::path::Path;
use std::sync::Arc;

use frust_drive::build_info::BuildMode;
use frust_drive::devices::{Device, Kind, Platform};
use frust_mcp::SharedBackend;
use frust_mcp::engine::RunTarget;

use crate::sanitize::console_safe;

/// The one `device` value that names a host preview rather than a device.
const DESKTOP: &str = "desktop";

/// The `console` note for a client that asked to build somewhere else, or
/// `None` when it asked for nothing or for the root the server already uses.
///
/// `server_root` is the **host's** project, read off its backend as this
/// launch starts (never a remembered copy — the host's open project can change
/// under a running server), and a client's
/// `launchArguments.projectRoot` is never honored: the listener is
/// unauthenticated loopback, and a client-chosen build directory is arbitrary
/// local code execution — `cargo` runs `build.rs`, proc macros, and a
/// `.cargo/config.toml [target.*.runner]` out of it. What the client asked for
/// is still *echoed*, so the deviation is visible rather than silent, and it is
/// echoed [`console_safe`]: the string is client-chosen and the console it
/// lands in may be a real terminal.
pub(crate) fn client_root_note(requested: Option<&str>, server_root: &Path) -> Option<String> {
    requested
        .map(str::trim)
        .filter(|root| !root.is_empty() && Path::new(root) != server_root)
        .map(|root| {
            format!(
                "Ignoring the launch configuration's 'projectRoot' ({}): this debug adapter is \
                 embedded in a workbench and builds only from that workbench's project ({}), \
                 because a client choosing the build directory over an unauthenticated local \
                 socket would run arbitrary local code. Restart the workbench in another \
                 directory to build from it.\n",
                console_safe(root),
                server_root.display()
            )
        })
}

/// Maps the `mode` field onto a [`BuildMode`], defaulting to `debug`.
///
/// All three modes are accepted, unlike `frust-mcp`'s `run_app` (which refuses
/// `release` outright): an MCP tool session exists to inspect and drive the
/// app, so a build with the devtools service compiled out is useless to it,
/// while a DAP session's core job — build, deploy, launch, stream output —
/// works in every mode. The one request that does need devtools
/// (`frustWidgetTree`) says so when it fails, and `launch` warns up front.
pub(crate) fn parse_mode(mode: Option<&str>) -> Result<BuildMode, String> {
    match mode.map(str::to_ascii_lowercase).as_deref() {
        None | Some("debug") => Ok(BuildMode::Debug),
        Some("profile") => Ok(BuildMode::Profile),
        Some("release") => Ok(BuildMode::Release),
        Some(other) => Err(format!(
            "unknown mode {other:?} in the launch configuration — use \"debug\", \"profile\", \
             or \"release\"."
        )),
    }
}

/// The human-readable name of a [`BuildMode`], for the launch banner.
pub(crate) fn mode_name(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

/// Maps the `device` field onto a [`RunTarget`].
///
/// An **absent** `device` resolves to `desktop`. `frust-mcp`'s `run_app` has no
/// default at all (its `target` is a required argument), and a DAP `launch`
/// cannot refuse a missing optional field the same way, so the default is the
/// one target that is always valid without discovery — and, unlike "the sole
/// connected device", it cannot silently change meaning when a phone is
/// plugged in. The resolved target is echoed to the Debug Console at launch so
/// a configuration that meant to name a device is visible immediately.
pub(crate) async fn resolve_target(
    backend: &SharedBackend,
    device: Option<&str>,
) -> Result<RunTarget, String> {
    let Some(requested) = device.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(RunTarget::Desktop);
    };
    if requested.eq_ignore_ascii_case(DESKTOP) {
        return Ok(RunTarget::Desktop);
    }
    let (devices, notes) = discover(backend).await;
    match_device(&devices, &notes, requested)
}

/// Device discovery, off the runtime: it shells out to `adb`/`xcrun`, either
/// of which an unresponsive device can stall without bound. A discovery task
/// that fails outright becomes a *note* rather than an error — the message
/// below is more useful for naming what was searched than for hiding it.
async fn discover(backend: &SharedBackend) -> (Vec<Device>, Vec<String>) {
    let backend = Arc::clone(backend);
    tokio::task::spawn_blocking(move || backend.list_devices())
        .await
        .unwrap_or_else(|err| (Vec::new(), vec![format!("device discovery failed: {err}")]))
}

/// Exact id first, then unique case-insensitive substring of id or name.
///
/// Ambiguity is never resolved by picking: two matches name both, so the
/// launch configuration can be made specific.
fn match_device(
    devices: &[Device],
    notes: &[String],
    requested: &str,
) -> Result<RunTarget, String> {
    let exact: Vec<&Device> = devices.iter().filter(|d| d.id == requested).collect();
    let matches: Vec<&Device> = if exact.is_empty() {
        let needle = requested.to_lowercase();
        devices
            .iter()
            .filter(|d| {
                d.id.to_lowercase().contains(&needle) || d.name.to_lowercase().contains(&needle)
            })
            .collect()
    } else {
        exact
    };

    match matches.as_slice() {
        [device] => run_target_for(device),
        [] => Err(format!(
            "no device matched {requested:?}. Known targets: \"desktop\"{}{}",
            devices
                .iter()
                .map(|d| format!(", {:?} ({})", d.id, d.name))
                .collect::<String>(),
            if notes.is_empty() {
                String::new()
            } else {
                format!(". Discovery notes: {}", notes.join("; "))
            }
        )),
        many => Err(format!(
            "{requested:?} matched {} devices: {}. Set 'device' to a full device id.",
            many.len(),
            many.iter()
                .map(|d| format!("{:?} ({})", d.id, d.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The target a device can be supervised as — a physical iOS device is none of
/// them, and says so rather than failing minutes later in the pipeline.
fn run_target_for(device: &Device) -> Result<RunTarget, String> {
    match (device.platform, device.kind) {
        (Platform::Android, _) => Ok(RunTarget::Android(device.clone())),
        (Platform::Ios, Kind::Simulator) => Ok(RunTarget::IosSimulator(device.clone())),
        (Platform::Ios, _) => Err(format!(
            "{:?} is a physical iOS device; this adapter can only supervise desktop, Android, \
             and iOS Simulator targets. Run it from the frust CLI instead.",
            device.id
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn android(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Android,
            kind: Kind::PhysicalDevice,
            os_version: Some("14".to_string()),
            connection_state: Some("device".to_string()),
        }
    }

    fn simulator(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: Some("17.5".to_string()),
            connection_state: None,
        }
    }

    #[test]
    fn the_mode_defaults_to_debug_and_accepts_all_three() {
        assert_eq!(parse_mode(None), Ok(BuildMode::Debug));
        assert_eq!(parse_mode(Some("DEBUG")), Ok(BuildMode::Debug));
        assert_eq!(parse_mode(Some("profile")), Ok(BuildMode::Profile));
        assert_eq!(parse_mode(Some("Release")), Ok(BuildMode::Release));
        let err = parse_mode(Some("fast")).expect_err("an unknown mode is refused");
        assert!(err.contains("fast"), "unhelpful: {err}");
    }

    /// A client-supplied `projectRoot` is never built from — but a client that
    /// sent one is told, by name, which directory was used instead.
    #[test]
    fn a_client_root_that_differs_from_the_servers_is_noted() {
        let server_root = Path::new("/home/me/app");

        let note = client_root_note(Some("/home/attacker/evil"), server_root)
            .expect("an ignored client root is surfaced");
        assert!(note.contains("/home/attacker/evil"), "{note}");
        assert!(note.contains("/home/me/app"), "{note}");
        assert!(note.contains("Ignoring"), "{note}");
    }

    /// Nothing to say when there is nothing to ignore: unset, blank (what a
    /// half-filled `launch.json` produces), or the very root already in use.
    #[test]
    fn a_matching_absent_or_blank_client_root_is_not_noted() {
        let server_root = Path::new("/home/me/app");

        assert!(client_root_note(None, server_root).is_none());
        assert!(client_root_note(Some("   "), server_root).is_none());
        assert!(client_root_note(Some("/home/me/app"), server_root).is_none());
    }

    /// The echoed path is client-chosen text reaching a console that may be a
    /// real terminal: an escape sequence in it must not survive the round
    /// trip.
    #[test]
    fn an_echoed_client_root_carries_no_control_characters() {
        let note = client_root_note(Some("/evil/\u{1b}[2J\u{7}"), Path::new("/home/me/app"))
            .expect("a differing root is noted");
        assert!(
            !note.contains('\u{1b}'),
            "an escape reached the console: {note}"
        );
        assert!(
            !note.contains('\u{7}'),
            "a bell reached the console: {note}"
        );
        // Exactly one line: the note's own trailing newline.
        assert_eq!(note.matches('\n').count(), 1, "{note}");
    }

    #[test]
    fn an_exact_id_wins_over_a_substring_match() {
        let devices = vec![android("pixel", "Pixel 8"), android("pixel-7", "Pixel 7")];
        let target = match_device(&devices, &[], "pixel").expect("the exact id resolves");
        assert_eq!(target.android_serial(), Some("pixel"));
    }

    #[test]
    fn a_unique_substring_of_the_name_resolves() {
        let devices = vec![
            android("ABCDEF", "Xiaomi 12"),
            simulator("AAAA-BBBB", "iPhone 15"),
        ];
        let target = match_device(&devices, &[], "xiaomi").expect("a unique name substring");
        assert_eq!(target.android_serial(), Some("ABCDEF"));

        let sim = match_device(&devices, &[], "iphone").expect("a unique simulator match");
        assert_eq!(sim.ios_simulator_udid(), Some("AAAA-BBBB"));
    }

    #[test]
    fn an_ambiguous_match_names_every_candidate_instead_of_picking() {
        let devices = vec![
            android("emulator-5554", "Pixel"),
            android("emulator-5556", "Pixel"),
        ];
        let err = match_device(&devices, &[], "emulator").expect_err("two matches are ambiguous");
        assert!(err.contains("emulator-5554"), "{err}");
        assert!(err.contains("emulator-5556"), "{err}");
    }

    #[test]
    fn no_match_lists_what_was_searched_including_discovery_notes() {
        let devices = vec![android("ABCDEF", "Xiaomi 12")];
        let notes = vec!["adb not found on PATH".to_string()];
        let err = match_device(&devices, &notes, "pixel").expect_err("nothing matched");
        assert!(err.contains("desktop"), "{err}");
        assert!(err.contains("ABCDEF"), "{err}");
        assert!(err.contains("adb not found on PATH"), "{err}");
    }

    #[test]
    fn a_physical_ios_device_is_refused_up_front() {
        let mut device = simulator("00008120-001", "Ed's iPhone");
        device.kind = Kind::PhysicalDevice;
        let err = match_device(std::slice::from_ref(&device), &[], "00008120-001")
            .expect_err("a physical iOS device cannot be supervised");
        assert!(err.contains("physical iOS device"), "{err}");
    }
}
