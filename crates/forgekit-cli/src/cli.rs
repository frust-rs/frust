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
    /// Scaffold a new app (spec §12.3 — v1 generates a desktop-preview
    /// Rust crate; `android/`/`ios/` platform projects land in spec Phase
    /// 2/3).
    Create {
        /// Target directory for the new project (may be `.`).
        dir: String,

        /// Reverse-DNS organization identifier, e.g. `dev.f0x`.
        #[arg(long, default_value = "com.example")]
        org: String,

        /// Project name; defaults to the target directory's basename.
        #[arg(long = "project-name", value_name = "NAME")]
        project_name: Option<String>,

        /// One-line project description.
        #[arg(long, default_value = "A new ForgeKit application.")]
        description: String,

        /// Overwrite a non-empty target directory.
        #[arg(long)]
        overwrite: bool,

        /// Override the embedded template directory (development only).
        #[arg(long = "template-dir", value_name = "PATH", hide = true)]
        template_dir: Option<String>,

        /// Override the computed path to the `forgekit` facade crate
        /// (development only; spec §12.3's temporary `forgekit_path`
        /// mechanism).
        #[arg(long = "forgekit-path", value_name = "PATH", hide = true)]
        forgekit_path: Option<String>,
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
    fn parses_create_dir_with_defaults() {
        let cli = Cli::parse_from(["forgekit", "create", "myapp"]);
        match cli.command {
            Command::Create { dir, org, project_name, overwrite, .. } => {
                assert_eq!(dir, "myapp");
                assert_eq!(org, "com.example");
                assert_eq!(project_name, None);
                assert!(!overwrite);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn parses_create_with_options() {
        let cli = Cli::parse_from([
            "forgekit",
            "create",
            "/tmp/x",
            "--project-name",
            "my_app",
            "--org",
            "dev.f0x",
            "--overwrite",
        ]);
        match cli.command {
            Command::Create { dir, org, project_name, overwrite, .. } => {
                assert_eq!(dir, "/tmp/x");
                assert_eq!(org, "dev.f0x");
                assert_eq!(project_name.as_deref(), Some("my_app"));
                assert!(overwrite);
            }
            other => panic!("expected Create, got {other:?}"),
        }
    }
}
