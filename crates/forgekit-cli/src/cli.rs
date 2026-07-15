//! Top-level clap parser (spec §12.1), modeled on `flutter_tools`' command surface.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "forgekit", version, about = "Tooling for ForgeKit apps (spec §12)")]
pub struct Cli {
    /// Target device id or name (prefix match allowed).
    #[arg(short = 'd', long = "device-id", global = true, value_name = "ID")]
    pub device_id: Option<String>,

    /// Increase verbosity; repeatable (-v, -vv, …).
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scaffold a new app (Rust + `android/` Gradle project + `ios/` Xcode project).
    Create {
        /// Target directory for the new project.
        dir: String,
    },
    /// Validate the ForgeKit toolchain (Rust targets, NDK, Android SDK, Xcode).
    Doctor,
    /// List connected devices, emulators, and simulators.
    Devices,
    /// Remove build outputs (cargo target dirs + Gradle/Xcode build dirs).
    Clean,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_debug_asserts() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_doctor() {
        let cli = Cli::parse_from(["forgekit", "doctor"]);
        assert!(matches!(cli.command, Command::Doctor));
    }

    #[test]
    fn parses_devices_with_global_flags() {
        let cli = Cli::parse_from(["forgekit", "-v", "-v", "devices", "-d", "pixel"]);
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.device_id.as_deref(), Some("pixel"));
        assert!(matches!(cli.command, Command::Devices));
    }

    #[test]
    fn parses_create_dir() {
        let cli = Cli::parse_from(["forgekit", "create", "myapp"]);
        match cli.command {
            Command::Create { dir } => assert_eq!(dir, "myapp"),
            other => panic!("expected Create, got {other:?}"),
        }
    }
}
