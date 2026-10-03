//! `frust create` — data-driven project scaffolding.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::CreatePlatformArgs;
use frust_drive::host_path;
use frust_drive::platform_wiring::{self, Report, SyncError};
use frust_drive::scaffold::{
    self, DesignSystemContext, FrustDependency, ScaffoldPlatform, TemplateContext, context,
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
    /// `--platforms` (comma-separated platform tags, `None` for the default
    /// set: android, ios, macos, windows, linux) and `--no-sync` (skip
    /// [`platform_wiring::sync`] after generating).
    pub platforms: CreatePlatformArgs,
    /// `--design-system`: scaffold a design-system crate (see
    /// `cli::Command::Create`'s doc comment) instead of an app.
    pub design_system: bool,
}

/// How `create` wires a generated project's Android/iOS projects — the
/// injection point that lets a test answer for cargo.
type WireFn<'a> = &'a dyn Fn(&Path) -> Result<Report, SyncError>;

pub fn run(args: CreateArgs) -> Result<u8> {
    run_with(args, &platform_wiring::sync)
}

/// [`run`] with the wiring step injected.
fn run_with(args: CreateArgs, sync: WireFn<'_>) -> Result<u8> {
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
        frust: resolve_frust_dependency(args.frust_path.as_deref())?,
        deeplink_scheme: args.deeplink_scheme,
        deeplink_host: args.deeplink_host,
    };

    // Parse the platforms list from the comma-separated string, or use DEFAULT.
    let platforms: Vec<ScaffoldPlatform> = match args.platforms.list.as_deref() {
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
    let wires_a_platform = platforms.contains(&ScaffoldPlatform::Android)
        || platforms.contains(&ScaffoldPlatform::Ios);
    if wires_a_platform {
        wire_platforms(&dest, args.platforms.no_sync, sync);
    }
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

/// Points the generated `android/`/`ios/` projects at the embedding modules
/// of the shell crates cargo resolves — the machine-local
/// `android/local.properties` key and `ios/FrustEmbedding` symlink — printing
/// what was written. Never fails `create`: the project is already on disk,
/// and `frust run`/`frust build` redo this step before every Android or iOS
/// build — so a skipped (`--no-sync`) or failed wiring is a note naming that
/// later path (a Gradle run before it fails naming the same commands).
fn wire_platforms(dest: &Path, no_sync: bool, sync: WireFn<'_>) {
    const LATER: &str = "`frust run` / `frust build` for Android or iOS wire them before invoking \
                         Gradle or Xcode";
    if no_sync {
        println!("Skipped wiring the Android/iOS projects (--no-sync); {LATER}.");
        return;
    }
    match sync(dest) {
        Ok(report) => {
            for line in report.lines() {
                println!("{line}");
            }
        }
        Err(err) => {
            println!("warning: could not wire the Android/iOS projects: {err}");
            println!("The project itself is complete; {LATER} once cargo can resolve them.");
        }
    }
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
        frust: resolve_frust_dependency(args.frust_path.as_deref())?,
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

/// Resolves the generated project's `frust` dependency: with `--frust-path`,
/// a path dependency on that checkout; without it, the crates.io release at
/// this CLI's own version.
///
/// An override accepts either the facade crate itself or its repo root
/// (see [`context::resolve_frust_crate_path`]); a repo-root value is
/// canonicalised to the nested facade crate directory with a printed note,
/// and anything else is a hard error naming both accepted shapes — a
/// repo-root value can otherwise silently bake `frust = { path =
/// "<repo-root>" }`, a virtual-workspace manifest with no `[package]` table
/// and a hard Cargo error at build time instead of scaffold time.
///
/// The override value is rendered through [`host_path::to_portable_string`]
/// before returning: on Windows, `Path::canonicalize()` returns a verbatim
/// `\\?\C:\...` path, which is both an invalid TOML escape sequence once
/// substituted into `Cargo.toml`'s `frust = { path = "..." }` and
/// unresolvable by a template's `..`-relative sibling joins — the portable,
/// forward-slash form Cargo/Gradle/Xcode all accept on Windows fixes both.
/// Identity on every other host.
fn resolve_frust_dependency(overridden: Option<&str>) -> Result<FrustDependency> {
    let Some(path) = overridden else {
        return Ok(FrustDependency::Registry {
            version: env!("CARGO_PKG_VERSION").to_string(),
        });
    };
    let resolved = context::resolve_frust_crate_path(Path::new(path))?;
    let resolved = host_path::to_portable_string(&resolved);
    if resolved != path {
        println!("note: --frust-path `{path}` normalised to `{resolved}`");
    }
    Ok(FrustDependency::Path(resolved))
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
            platforms: CreatePlatformArgs {
                list: None,
                no_sync: true,
            },
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
        args.platforms.list = Some("web".to_string());

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
        args.platforms.list = Some("android,web".to_string());

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
        args.platforms.list = Some("android,bogus".to_string());

        let err = run(args).unwrap_err();
        assert!(err.to_string().contains("unknown platform tag"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = std::fs::remove_dir_all(&dest);
    }

    /// Without `--no-sync`, `create` wires what it generated: the
    /// machine-local `local.properties` key gets the resolved module
    /// directory, `ios/FrustEmbedding` links to the resolved package — here
    /// answered by a stub standing in for cargo — and the tracked
    /// `gradle.properties` names no path.
    #[cfg(unix)]
    #[test]
    fn run_wires_the_generated_platform_projects() {
        use frust_drive::packages::StubLocator;

        let root = unique_temp_dir("wired");
        let shells = root.join("checkout");
        let android = shells.join("frust-shell-android");
        let ios = shells.join("frust-shell-ios");
        std::fs::create_dir_all(android.join(platform_wiring::ANDROID_EMBEDDING_REL)).unwrap();
        std::fs::create_dir_all(ios.join(platform_wiring::IOS_EMBEDDING_REL)).unwrap();
        let shells = shells.canonicalize().unwrap();
        let stub = StubLocator::new()
            .with(
                platform_wiring::ANDROID_SHELL_PACKAGE,
                shells.join("frust-shell-android"),
            )
            .with(
                platform_wiring::IOS_SHELL_PACKAGE,
                shells.join("frust-shell-ios"),
            );

        let dest = root.join("app");
        let mut args = base_args(&dest);
        args.platforms.no_sync = false;
        run_with(args, &|dir| platform_wiring::sync_with(&stub, dir)).unwrap();

        let properties =
            std::fs::read_to_string(dest.join(platform_wiring::LOCAL_PROPERTIES)).unwrap();
        assert!(
            properties.contains(&format!(
                "frust.embedding.dir={}\n",
                shells
                    .join("frust-shell-android")
                    .join(platform_wiring::ANDROID_EMBEDDING_REL)
                    .display()
            )),
            "{properties}"
        );
        let gradle_properties =
            std::fs::read_to_string(dest.join(platform_wiring::GRADLE_PROPERTIES)).unwrap();
        assert!(
            !gradle_properties.contains(platform_wiring::EMBEDDING_DIR_KEY),
            "{gradle_properties}"
        );
        assert_eq!(
            std::fs::read_link(dest.join(platform_wiring::IOS_EMBEDDING_LINK)).unwrap(),
            shells
                .join("frust-shell-ios")
                .join(platform_wiring::IOS_EMBEDDING_REL)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `--no-sync` never reaches the wiring step, and a wiring failure
    /// (offline, say) still leaves a successful `create` — with no
    /// `local.properties` yet, so Gradle fails naming `frust run`/`frust
    /// build` until one of them writes it.
    #[test]
    fn no_sync_skips_wiring_and_a_failed_wiring_does_not_fail_create() {
        let calls = std::cell::Cell::new(0);
        let failing = |dir: &Path| {
            calls.set(calls.get() + 1);
            Err(SyncError::Locate(
                frust_drive::packages::PackagesError::NoManifest {
                    project_dir: dir.to_path_buf(),
                },
            ))
        };

        let skipped = unique_temp_dir("no-sync");
        assert_eq!(run_with(base_args(&skipped), &failing).unwrap(), 0);
        assert_eq!(calls.get(), 0, "--no-sync must not wire");

        let offline = unique_temp_dir("sync-fails");
        let mut args = base_args(&offline);
        args.platforms.no_sync = false;
        assert_eq!(run_with(args, &failing).unwrap(), 0);
        assert_eq!(calls.get(), 1);
        assert!(!offline.join(platform_wiring::LOCAL_PROPERTIES).exists());
        let settings =
            std::fs::read_to_string(offline.join("android/settings.gradle.kts")).unwrap();
        assert!(
            settings.contains("`frust run` or `frust build`"),
            "the missing-key failure names the fix:\n{settings}"
        );

        // A scaffold with neither Android nor iOS has nothing to wire.
        let web_only = unique_temp_dir("web-only");
        let mut args = base_args(&web_only);
        args.platforms = CreatePlatformArgs {
            list: Some("web".to_string()),
            no_sync: false,
        };
        assert_eq!(run_with(args, &failing).unwrap(), 0);
        assert_eq!(calls.get(), 1);

        for dir in [skipped, offline, web_only] {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
