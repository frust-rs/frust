//! `frust create` — data-driven project scaffolding.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use frust_drive::scaffold::{
    self, DesignSystemContext, ScaffoldPlatform, TemplateContext, context,
};

/// Parsed + defaulted arguments for `frust create` (mirrors
/// `cli::Command::Create`; kept separate so `scaffold` stays decoupled
/// from `clap`).
pub struct CreateArgs {
    pub dir: String,
    pub org: String,
    pub project_name: Option<String>,
    pub description: String,
    pub overwrite: bool,
    /// `--template-dir` (undocumented, development only).
    pub template_dir: Option<String>,
    /// `--frust-path` (undocumented, development only).
    pub frust_path: Option<String>,
    /// `--deeplink-scheme`.
    pub deeplink_scheme: Option<String>,
    /// `--deeplink-host`.
    pub deeplink_host: Option<String>,
    /// `--arch`: a `scaffold::KNOWN_ARCHES` tag string (e.g.
    /// `"clean-signals"`), or `None` for the default template. Kept a plain
    /// `String` rather than `crate::cli::ArchArg` so `scaffold` stays
    /// decoupled from `clap` (mirrors every other field here).
    pub arch: Option<String>,
    /// `--platforms`: comma-separated platform tags, or `None` for the
    /// default set (android, ios, macos, windows, linux). Kept a plain
    /// `String` (the comma-separated list) rather than a parsed `Vec` so
    /// `scaffold` stays decoupled from `clap`.
    pub platforms: Option<String>,
    /// `--design-system`: scaffold a design-system crate (see
    /// `cli::Command::Create`'s doc comment) instead of an app.
    pub design_system: bool,
}

pub fn run(args: CreateArgs) -> Result<u8> {
    if args.design_system {
        return run_design_system(args);
    }

    let dest = PathBuf::from(&args.dir);
    let project_name = match args.project_name {
        Some(name) => name,
        None => infer_project_name(&dest).with_context(|| {
            format!(
                "could not derive a project name from `{}`; pass --project-name",
                args.dir
            )
        })?,
    };
    scaffold::validate_project_name(&project_name)?;
    if let Some(scheme) = args.deeplink_scheme.as_deref() {
        scaffold::validate_deeplink_scheme(scheme)
            .with_context(|| format!("invalid --deeplink-scheme `{scheme}`"))?;
    }

    let ctx = TemplateContext {
        title_case_name: scaffold::title_case(&project_name),
        project_name: project_name.clone(),
        org: args.org,
        description: args.description,
        frust_version: env!("CARGO_PKG_VERSION").to_string(),
        frust_path: resolve_frust_path(args.frust_path.as_deref())?,
        deeplink_scheme: args.deeplink_scheme,
        deeplink_host: args.deeplink_host,
    };

    // Parse the platforms list from the comma-separated string, or use DEFAULT.
    let platforms: Vec<ScaffoldPlatform> = match args.platforms.as_deref() {
        Some(list) => {
            let tags: Vec<&str> = list.split(',').map(|s| s.trim()).collect();
            let mut platforms = Vec::new();
            for tag in tags {
                let platform = ScaffoldPlatform::from_tag(tag).ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown platform tag `{tag}` (expected one of: {})",
                        scaffold::known_platform_tags().join(", ")
                    )
                })?;
                platforms.push(platform);
            }
            platforms
        }
        None => ScaffoldPlatform::DEFAULT.to_vec(),
    };

    let template_dir_override = args.template_dir.as_deref().map(Path::new);
    let written = scaffold::generate_with_platforms(
        &dest,
        &ctx,
        template_dir_override,
        args.overwrite,
        args.arch.as_deref(),
        &platforms,
    )?;

    println!("Created {} file(s) in {}", written.len(), dest.display());
    println!();
    println!("All done! `{project_name}` is ready.");
    println!();
    println!("To run your app:");
    if args.dir != "." {
        println!("  cd {}", args.dir);
    }
    println!("  cargo run");

    Ok(0)
}

/// `frust create --design-system`: scaffolds a design-system crate instead
/// of an app. A design-system crate is a plain library with no platform
/// project, so this bypasses the app-only options entirely rather than
/// threading dummy values through them (`context::DesignSystemContext` is a
/// strict subset of `TemplateContext`'s fields — see that struct's doc).
/// `--deeplink-scheme`/`--deeplink-host`/`--arch` are rejected outright if
/// passed alongside `--design-system`, since silently ignoring them would
/// look like they took effect; `--org`/`--description` don't apply either
/// but are harmlessly unused (both carry a default value already, so a
/// caller can't tell whether one was explicitly passed).
fn run_design_system(args: CreateArgs) -> Result<u8> {
    if args.deeplink_scheme.is_some() || args.deeplink_host.is_some() {
        bail!(
            "--deeplink-scheme/--deeplink-host don't apply to --design-system (a design-system \
             crate has no platform manifest)"
        );
    }
    if args.arch.is_some() {
        bail!(
            "--arch doesn't apply to --design-system (a design-system crate has no app \
             architecture variant)"
        );
    }

    let dest = PathBuf::from(&args.dir);
    let name = match args.project_name {
        Some(name) => name,
        None => infer_project_name(&dest).with_context(|| {
            format!(
                "could not derive a crate name from `{}`; pass --project-name",
                args.dir
            )
        })?,
    };
    scaffold::validate_project_name(&name)?;

    let ctx = DesignSystemContext {
        name: name.clone(),
        frust_version: env!("CARGO_PKG_VERSION").to_string(),
        frust_path: resolve_frust_path(args.frust_path.as_deref())?,
    };

    let template_dir_override = args.template_dir.as_deref().map(Path::new);
    let written =
        scaffold::generate_design_system(&dest, &ctx, template_dir_override, args.overwrite)?;

    println!("Created {} file(s) in {}", written.len(), dest.display());
    println!();
    println!("All done! `{name}` is ready.");
    println!();
    println!("To build it:");
    if args.dir != "." {
        println!("  cd {}", args.dir);
    }
    println!("  cargo build");

    Ok(0)
}

/// Resolves `--frust-path`, defaulting to this repo's `crates/frust`
/// (a temporary mechanism until Frust crates are
/// published), derived from `frust-cli`'s own compile-time manifest
/// directory.
///
/// An override accepts either the facade crate itself or its repo root
/// (see [`context::resolve_frust_crate_path`]); a repo-root value is
/// canonicalised to the nested facade crate directory with a printed note,
/// and anything else is a hard error naming both accepted shapes — a
/// repo-root value can otherwise silently bake `frust = { path =
/// "<repo-root>" }`, a virtual-workspace manifest with no `[package]` table
/// and a hard Cargo error at build time instead of scaffold time.
fn resolve_frust_path(overridden: Option<&str>) -> Result<String> {
    if let Some(path) = overridden {
        let resolved = context::resolve_frust_crate_path(Path::new(path))?;
        let resolved = resolved.to_string_lossy().into_owned();
        if resolved != path {
            println!("note: --frust-path `{path}` normalised to `{resolved}`");
        }
        return Ok(resolved);
    }
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frust");
    Ok(raw
        .canonicalize()
        .unwrap_or(raw)
        .to_string_lossy()
        .into_owned())
}

/// Infers a project name from `dir`'s basename, resolving `.`/`..`
/// lexically — the target need not exist yet, so this can't use
/// `fs::canonicalize`.
fn infer_project_name(dir: &Path) -> Result<String> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        cwd.join(dir)
    };
    let normalized = normalize_lexically(&absolute);
    normalized
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .with_context(|| {
            format!(
                "`{}` has no basename to use as a project name",
                dir.display()
            )
        })
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_project_name_uses_dir_basename() {
        assert_eq!(
            infer_project_name(Path::new("/tmp/my_app")).unwrap(),
            "my_app"
        );
    }

    #[test]
    fn infer_project_name_resolves_dot_to_cwd_basename() {
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(infer_project_name(Path::new(".")).unwrap(), expected);
    }

    #[test]
    fn infer_project_name_resolves_trailing_parent_dir() {
        assert_eq!(
            infer_project_name(Path::new("/tmp/foo/../my_app")).unwrap(),
            "my_app"
        );
    }

    fn unique_temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-create-cmd-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn base_args(dir: &Path) -> CreateArgs {
        CreateArgs {
            dir: dir.to_string_lossy().into_owned(),
            org: "dev.f0x".to_string(),
            project_name: Some("my_app".to_string()),
            description: "A new Frust application.".to_string(),
            overwrite: false,
            template_dir: None,
            frust_path: None,
            deeplink_scheme: None,
            deeplink_host: None,
            arch: None,
            platforms: None,
            design_system: false,
        }
    }

    /// An invalid `--deeplink-scheme` (a full URL, not a bare
    /// scheme) is rejected before any file is written.
    #[test]
    fn run_rejects_invalid_deeplink_scheme_before_writing_any_file() {
        let dest = unique_temp_dir("invalid-deeplink-scheme");
        let mut args = base_args(&dest);
        args.deeplink_scheme = Some("myapp://".to_string());

        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("--deeplink-scheme"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn run_accepts_valid_deeplink_scheme() {
        let dest = unique_temp_dir("valid-deeplink-scheme");
        let mut args = base_args(&dest);
        args.deeplink_scheme = Some("myapp".to_string());

        assert!(run(args).is_ok());
        assert!(dest.join("Cargo.toml").exists());
        let manifest =
            std::fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(manifest.contains("android:scheme=\"myapp\""), "{manifest}");

        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn run_design_system_scaffolds_a_library_crate() {
        let dest = unique_temp_dir("design-system");
        let mut args = base_args(&dest);
        args.design_system = true;

        assert!(run(args).is_ok());
        assert!(dest.join("Cargo.toml").exists());
        assert!(dest.join("src/lib.rs").exists());
        assert!(dest.join("src/tokens.rs").exists());
        assert!(!dest.join("android").exists());
        assert!(!dest.join("frust.toml").exists());

        let cargo_toml = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("name = \"my_app\""), "{cargo_toml}");

        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn run_design_system_rejects_deeplink_scheme() {
        let dest = unique_temp_dir("design-system-rejects-deeplink");
        let mut args = base_args(&dest);
        args.design_system = true;
        args.deeplink_scheme = Some("myapp".to_string());

        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("--design-system"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn run_design_system_rejects_arch() {
        let dest = unique_temp_dir("design-system-rejects-arch");
        let mut args = base_args(&dest);
        args.design_system = true;
        args.arch = Some("clean-signals".to_string());

        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("--design-system"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    /// Default (None) platforms produces the current byte-identical output
    /// by using ScaffoldPlatform::DEFAULT.
    #[test]
    fn run_default_platforms_unchanged() {
        let dest = unique_temp_dir("default-platforms");
        let args = base_args(&dest);

        assert!(run(args).is_ok());
        assert!(dest.join("Cargo.toml").exists());
        // DEFAULT includes android, ios, macos, windows, linux but NOT web
        assert!(dest.join("android").exists());
        assert!(dest.join("ios").exists());
        assert!(!dest.join("web").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    /// `--platforms web` adds the web host page alongside native platforms.
    #[test]
    fn run_with_platforms_web_includes_web_files() {
        let dest = unique_temp_dir("platforms-web");
        let mut args = base_args(&dest);
        args.platforms = Some("web".to_string());

        assert!(run(args).is_ok());
        assert!(dest.join("Cargo.toml").exists());
        assert!(
            dest.join("web/index.html").exists(),
            "web/index.html must exist"
        );
        assert!(
            dest.join("web/frust_web.js").exists(),
            "web/frust_web.js must exist"
        );

        let _ = std::fs::remove_dir_all(&dest);
    }

    /// `--platforms android,web` includes both android and web.
    #[test]
    fn run_with_platforms_android_web() {
        let dest = unique_temp_dir("platforms-android-web");
        let mut args = base_args(&dest);
        args.platforms = Some("android,web".to_string());

        assert!(run(args).is_ok());
        assert!(dest.join("Cargo.toml").exists());
        assert!(dest.join("android").exists());
        assert!(dest.join("web/index.html").exists());
        assert!(!dest.join("ios").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    /// An unknown platform tag is rejected before any file is written.
    #[test]
    fn run_rejects_unknown_platform_tag() {
        let dest = unique_temp_dir("unknown-platform");
        let mut args = base_args(&dest);
        args.platforms = Some("android,bogus".to_string());

        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("unknown platform tag"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }
}
