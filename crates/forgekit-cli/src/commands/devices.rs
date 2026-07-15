use crate::devices::{self, Device, Kind, Platform};
use crate::process::RealProcessRunner;
use anyhow::Result;

/// Discovers and prints connected devices/emulators/simulators. Always exits
/// `0` — an empty result is a valid state, not a failure.
pub fn run(verbose: bool) -> Result<u8> {
    let runner = RealProcessRunner;
    let discoverers = devices::default_discoverers();
    let (devices, notes) = devices::discover_all(&runner, &discoverers);

    if devices.is_empty() {
        println!("No devices found.");
        println!();
        println!("Run `forgekit doctor` to check your toolchain, or:");
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
    }

    Ok(0)
}

fn print_table(devices: &[Device]) {
    let name_width = devices.iter().map(|d| d.name.len()).max().unwrap_or(4).max(4);
    let id_width = devices.iter().map(|d| d.id.len()).max().unwrap_or(2).max(2);
    for device in devices {
        println!(
            "{:name_width$}  {:id_width$}  {:8}  {}",
            device.name,
            device.id,
            platform_label(device.platform),
            kind_label(device.kind),
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
