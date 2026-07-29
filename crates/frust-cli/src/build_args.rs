//! The clap half of the build-flag funnel.
//!
//! `frust-drive` owns the clap-free [`frust_drive::build_info::BuildArgs`]
//! and the [`frust_drive::build_info::BuildInfo`] validation it feeds; this
//! module holds only the `#[derive(clap::Args)]` surface `run`/`build`
//! `#[command(flatten)]` and the [`BuildArgs::into_drive`] conversion at the
//! command-handler boundary — clap never reaches `frust-drive` (see
//! `docs/ARCHITECTURE.md`).

/// `#[command(flatten)]`-able flags shared by every command that produces a
/// build. Mirrors [`frust_drive::build_info::BuildArgs`] field-for-field and
/// converts into it via [`BuildArgs::into_drive`].
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BuildArgs {
    /// Build in debug mode (`dev` cargo profile).
    #[arg(long)]
    pub debug: bool,
    /// Build in profile mode (release opts + debug symbols + tracing).
    #[arg(long)]
    pub profile: bool,
    /// Build in release mode (LTO, stripped, `panic=abort`).
    #[arg(long)]
    pub release: bool,
    /// Product flavor to build (maps to a Gradle flavor / Xcode scheme).
    #[arg(long)]
    pub flavor: Option<String>,
    /// Compile-time app config, `KEY=VALUE`; repeatable.
    #[arg(long = "define", value_name = "KEY=VALUE")]
    pub defines: Vec<String>,
    /// Semantic version string embedded in the build (e.g. `1.2.3`).
    #[arg(long = "build-name", value_name = "VER")]
    pub build_name: Option<String>,
    /// Monotonically increasing build number embedded in the build; must be
    /// `>= 1`.
    #[arg(long = "build-number", value_name = "N")]
    pub build_number: Option<u32>,
}

impl BuildArgs {
    /// Converts the parsed clap flags into the clap-free `frust-drive` funnel
    /// input — the one boundary where the two mirror structs meet.
    pub fn into_drive(self) -> frust_drive::build_info::BuildArgs {
        frust_drive::build_info::BuildArgs {
            debug: self.debug,
            profile: self.profile,
            release: self.release,
            flavor: self.flavor,
            defines: self.defines,
            build_name: self.build_name,
            build_number: self.build_number,
        }
    }
}
