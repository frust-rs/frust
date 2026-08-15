//! macOS `.app` assembly plus the optional `codesign` step.
//!
//! Layout (the shared desktop-bundle contract):
//!
//! ```text
//! dist/macos/<Display Name>.app/
//!   Contents/
//!     Info.plist
//!     MacOS/<binary>
//!     Resources/<binary>.icns
//! ```
//!
//! The `.icns` is named after the binary because that is what the scaffolded
//! `Info.plist`'s `CFBundleIconFile` names — the two have to agree, and the
//! plist is the file a user may have hand-edited, so the icon follows it.
//!
//! `Info.plist` is copied from the project's own `macos/Info.plist` whenever
//! it exists (rendered at `frust create` time, hand-editable ever after). A
//! project scaffolded before that template existed gets a minimal plist
//! generated from `frust.toml` instead — enough keys for macOS to launch and
//! name the app — rather than a failed build. Nothing rewrites an existing
//! plist: `[macos] minimum-system-version` feeds the *generated* one only, so
//! the file in the project always wins.

use std::path::{Path, PathBuf};

use crate::build_info::BuildInfo;
use crate::icons;
use crate::process::{ProcessRunner, tail_lines};

use super::bundle::{copy_file, create_dir, icon_source, prepare_dir, record_icons, write_file};
use super::config::DesktopConfig;
use super::{BundleNote, BundleReport, DesktopBuildError, DesktopBundleTarget};

/// How many trailing `codesign` output lines a failure reports.
const FAILURE_TAIL_LINES: usize = 20;

/// Fallback `CFBundleShortVersionString` when `--build-name` isn't given —
/// the same `0.1.0` a scaffolded `Info.plist` and `Cargo.toml` carry.
const DEFAULT_SHORT_VERSION: &str = "0.1.0";

/// Assembles the `.app` bundle for `config` around the compiled `binary`.
pub(super) fn assemble(
    project_dir: &Path,
    info: &BuildInfo,
    config: &DesktopConfig,
    binary: &Path,
    notes: &mut Vec<BundleNote>,
) -> Result<BundleReport, DesktopBuildError> {
    let root = app_path(project_dir, &config.display_name);
    prepare_dir(&root, project_dir)?;

    let contents = root.join("Contents");
    let resources = contents.join("Resources");
    create_dir(&contents.join("MacOS"))?;
    create_dir(&resources)?;

    let mut artifacts = Vec::new();

    let executable = contents.join("MacOS").join(&config.binary_name);
    copy_file(binary, &executable)?;
    artifacts.push(executable.clone());

    if let Some(source) = icon_source(config, notes) {
        let icns = resources.join(format!("{}.icns", config.binary_name));
        let generated = icons::generate_icns(&source, &icns);
        artifacts.extend(record_icons(&source, generated, notes));
    }

    let plist = contents.join("Info.plist");
    let project_plist = project_dir.join("macos").join("Info.plist");
    if project_plist.is_file() {
        copy_file(&project_plist, &plist)?;
    } else {
        notes.push(BundleNote::GeneratedInfoPlist);
        write_file(&plist, &info_plist(config, info))?;
    }
    artifacts.push(plist);

    Ok(BundleReport {
        target: DesktopBundleTarget::Macos,
        root,
        executable,
        artifacts,
        notes: Vec::new(),
    })
}

/// Codesigns `app` with `[macos] signing-identity`, or records
/// [`BundleNote::Unsigned`] when none is configured — the whole of the
/// "invoked iff configured" contract lives in this one branch.
///
/// `macos/app.entitlements` is passed when the project has one (the scaffold
/// ships it for exactly this call). Notarization is deliberately not
/// automated: it needs credentials and a network round trip, and stays a
/// documented manual step.
pub(super) fn codesign(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    config: &DesktopConfig,
    app: &Path,
    notes: &mut Vec<BundleNote>,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), DesktopBuildError> {
    let Some(identity) = config.macos_signing_identity.clone() else {
        notes.push(BundleNote::Unsigned);
        return Ok(());
    };

    let entitlements = project_dir.join("macos").join("app.entitlements");
    let entitlements = entitlements.is_file().then(|| path_arg(&entitlements));
    let app_arg = path_arg(app);

    // `--force` so re-signing an already-signed bundle (every rebuild) works
    // rather than failing on the existing signature.
    let mut args: Vec<&str> = vec!["--force", "--sign", identity.as_str()];
    if let Some(entitlements) = entitlements.as_deref() {
        args.push("--entitlements");
        args.push(entitlements);
    }
    args.push(&app_arg);

    let mut prefixed = |line: &str| on_line(&format!("[codesign] {line}"));
    let out = runner
        .run_streaming("codesign", &args, Some(project_dir), &[], &mut prefixed)
        .map_err(|err| DesktopBuildError::CodesignSpawn {
            reason: format!("{err:#}"),
        })?;
    if !out.success {
        return Err(DesktopBuildError::CodesignFailed {
            identity,
            tail: tail_lines(&out.stderr, FAILURE_TAIL_LINES),
        });
    }

    notes.push(BundleNote::Signed { identity });
    Ok(())
}

/// A path as a command-line argument. Lossy by necessity — the
/// [`ProcessRunner`] seam speaks `&str` — which is safe here because every
/// path involved is derived from the project directory the caller passed in.
fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The minimal `Info.plist` generated for a project with no `macos/Info.plist`
/// of its own: the keys macOS needs to launch, name and identify the bundle,
/// in the same key order and tab indentation the scaffolded template uses.
fn info_plist(config: &DesktopConfig, info: &BuildInfo) -> String {
    let short_version = info
        .build_name
        .clone()
        .unwrap_or_else(|| DEFAULT_SHORT_VERSION.to_string());
    let version = info
        .build_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "1".to_string());

    let entries: [(&str, String); 11] = [
        ("CFBundleDevelopmentRegion", "en".to_string()),
        ("CFBundleDisplayName", config.display_name.clone()),
        ("CFBundleExecutable", config.binary_name.clone()),
        ("CFBundleIconFile", config.binary_name.clone()),
        ("CFBundleIdentifier", config.identifier.clone()),
        ("CFBundleInfoDictionaryVersion", "6.0".to_string()),
        ("CFBundleName", config.display_name.clone()),
        ("CFBundlePackageType", "APPL".to_string()),
        ("CFBundleShortVersionString", short_version),
        ("CFBundleVersion", version),
        (
            "LSMinimumSystemVersion",
            config.macos_minimum_system_version.clone(),
        ),
    ];

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    for (key, value) in entries {
        out.push_str(&format!(
            "\t<key>{key}</key>\n\t<string>{}</string>\n",
            xml_escape(&value)
        ));
    }
    out.push_str("\t<key>NSHighResolutionCapable</key>\n\t<true/>\n</dict>\n</plist>\n");
    out
}

/// XML text-node escaping for the five predefined entities — a display name
/// is free-form user text and can carry any of them.
fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// The `.app` directory an assembly for `config` produces, without running
/// one — for a caller that needs to name the bundle ahead of time (an
/// installer step, a UI label).
pub(super) fn app_path(project_dir: &Path, display_name: &str) -> PathBuf {
    DesktopBundleTarget::Macos
        .dist_dir(project_dir)
        .join(format!("{display_name}.app"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::manifest;

    fn config(toml: &str) -> DesktopConfig {
        DesktopConfig::resolve(
            Path::new("/projects/my_app"),
            &manifest::parse(toml).unwrap(),
        )
        .unwrap()
    }

    fn info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                build_name: Some("2.1.0".to_string()),
                build_number: Some(42),
                ..BuildArgs::default()
            },
            BuildMode::Release,
        )
        .unwrap()
    }

    #[test]
    fn the_generated_plist_carries_every_launch_critical_key() {
        let plist = info_plist(
            &config(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"My App\"\n\n[macos]\nminimum-system-version = \"12.3\"\n",
            ),
            &info(),
        );
        for expected in [
            "<key>CFBundleDisplayName</key>\n\t<string>My App</string>",
            "<key>CFBundleExecutable</key>\n\t<string>my_app</string>",
            "<key>CFBundleIdentifier</key>\n\t<string>dev.f0x.my_app</string>",
            "<key>CFBundleShortVersionString</key>\n\t<string>2.1.0</string>",
            "<key>CFBundleVersion</key>\n\t<string>42</string>",
            "<key>LSMinimumSystemVersion</key>\n\t<string>12.3</string>",
            "<key>NSHighResolutionCapable</key>\n\t<true/>",
        ] {
            assert!(plist.contains(expected), "missing {expected} in\n{plist}");
        }
        assert!(plist.starts_with("<?xml version=\"1.0\""), "{plist}");
        assert!(plist.trim_end().ends_with("</plist>"), "{plist}");
    }

    #[test]
    fn a_display_name_with_xml_syntax_is_escaped() {
        let plist = info_plist(
            &config(
                "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
                 [desktop]\nname = \"Ben & Jerry's <App>\"\n",
            ),
            &info(),
        );
        assert!(
            plist.contains("<string>Ben &amp; Jerry&apos;s &lt;App&gt;</string>"),
            "{plist}"
        );
    }

    #[test]
    fn the_app_directory_is_named_after_the_display_name() {
        assert_eq!(
            app_path(Path::new("/projects/my_app"), "My App"),
            Path::new("/projects/my_app/dist/macos/My App.app")
        );
    }
}
