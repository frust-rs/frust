use anyhow::Result;
use frust_drive::devices::{self, Device, Kind, Platform};
use frust_drive::process::ProcessRunner;

/// Discovers and prints connected devices/emulators/simulators. Always exits
/// `0` — an empty result is a valid state, not a failure. The process runner
/// is injected by `commands::dispatch` (the CLI's one `Real` construction
/// site).
pub fn run_in(runner: &dyn ProcessRunner, verbose: bool) -> Result<u8> {
    let discoverers = devices::default_discoverers();
    let (devices, notes) = devices::discover_all(runner, &discoverers);

    if devices.is_empty() {
        println!("No devices found.");
        println!();
        println!("Run `frust doctor` to check your toolchain, or:");
        println!("  - start an Android emulator (Android Studio > Device Manager)");
        println!("  - boot an iOS simulator (`open -a Simulator`)");
        println!("  - connect a physical device and trust this computer");
    } else {
        print_table(&devices);
    }

    if verbose {
        for note in &notes {
            println!("[note] {note}");
        }
    } else if !notes.is_empty() {
        // A device skipped by discovery (unpaired, unreachable, missing
        // tool) must never be a silent absence — point at the details.
        println!();
        println!(
            "{} discovery note(s) — rerun with -v for details",
            notes.len()
        );
    }

    Ok(0)
}

fn print_table(devices: &[Device]) {
    let name_width = devices
        .iter()
        .map(|d| d.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let id_width = devices.iter().map(|d| d.id.len()).max().unwrap_or(2).max(2);
    for device in devices {
        // e.g. devicectl's tunnelState — "disconnected" is still a
        // targetable paired device (the tunnel comes up lazily on
        // install/launch), so it's informational, not an availability flag.
        let connection = device
            .connection_state
            .as_deref()
            .map(|state| format!("  [{state}]"))
            .unwrap_or_default();
        println!(
            "{:name_width$}  {:id_width$}  {:8}  {}{}",
            device.name,
            device.id,
            platform_label(device.platform),
            kind_label(device.kind),
            connection,
        );
    }
}

fn platform_label(platform: Platform) -> &'static str {
    match platform {
        Platform::Android => "android",
        Platform::Ios => "ios",
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::PhysicalDevice => "device",
        Kind::Emulator => "emulator",
        Kind::Simulator => "simulator",
    }
}
