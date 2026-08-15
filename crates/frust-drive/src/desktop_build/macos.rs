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
//!
//! …which is exactly why a copied plist is **read back** ([`reconcile`]): the
//! executable and the `.icns` this module writes are named from the resolved
//! manifest, so a plist naming something else produces a bundle that doesn't
//! launch or shows no icon. Three keys are compared, through `plutil -extract
//! <key> raw` over the injected [`ProcessRunner`] — the same tool and contract
//! `crate::ios_run::bundle_id` reads a built bundle's identifier with, since
//! parsing a possibly-binary plist by hand is not something this pipeline
//! should own. `CFBundleExecutable` disagreeing is fatal; the other two are
//! notes; and a plist that can't be read is one note and nothing else. See the
//! module header of [`super`] for why the line falls there.

use std::path::{Path, PathBuf};

use crate::build_info::BuildInfo;
use crate::icons;
use crate::process::{ProcessRunner, tail_lines};

use super::bundle::{copy_file, create_dir, icon_source, prepare_dir, record_icons, write_file};
use super::config::DesktopConfig;
use super::{BundleNote, BundleReport, DesktopBuildError, DesktopBundleTarget};

/// How many trailing `codesign` output lines a failure reports.
const FAILURE_TAIL_LINES: usize = 20;

/// Identity prefixes Apple itself issues. `--timestamp` is only ever passed
/// for one of these: a secure timestamp requires reaching Apple's timestamp
/// authority over the network, and an ad-hoc or self-signed local identity
/// (`-`, a hand-rolled "My Self Signed" cert) has to keep signing possible
/// fully offline — forcing `--timestamp` there would turn every such build
/// into a network dependency for no verification benefit, since nothing
/// trusts that signature's chain anyway.
const APPLE_ISSUED_IDENTITY_PREFIXES: &[&str] = &[
    "Developer ID Application:",
    "Apple Distribution:",
    "3rd Party Mac Developer Application:",
];

/// The macOS plist reader. Ships with the base system rather than with Xcode,
/// so a bare `PATH` lookup is enough (`crate::ios_run::bundle_id`'s precedent).
const PLUTIL: &str = "plutil";

/// The three keys a copied plist is reconciled on.
const KEY_EXECUTABLE: &str = "CFBundleExecutable";
const KEY_IDENTIFIER: &str = "CFBundleIdentifier";
const KEY_ICON_FILE: &str = "CFBundleIconFile";

/// Where an icon-name mismatch was declared, for
/// [`BundleNote::IconIdentityMismatch`]'s message.
const ICON_KEY_SOURCE: &str = "`macos/Info.plist`'s CFBundleIconFile";

/// The extension `CFBundleIconFile` may or may not carry — Apple accepts the
/// icon name with it and without it, so both spellings compare equal here.
const ICNS_EXTENSION: &str = ".icns";

/// Fallback `CFBundleShortVersionString` when `--build-name` isn't given —
/// the same `0.1.0` a scaffolded `Info.plist` and `Cargo.toml` carry.
const DEFAULT_SHORT_VERSION: &str = "0.1.0";

/// Assembles the `.app` bundle for `config` around the compiled `binary`.
///
/// `runner` is used for one thing only: reading a *copied* `Info.plist` back
/// through `plutil` (see [`reconcile`]). An assembly over a generated plist
/// spawns nothing.
pub(super) fn assemble(
    runner: &dyn ProcessRunner,
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

    // `None` unless an `.icns` was really written this run: `record_icons`
    // yields no paths for a source the icon pipeline rejected, and there is
    // already a note saying why. Nothing is compared against a file that
    // wasn't produced.
    let mut generated_icon = None;
    if let Some(source) = icon_source(config, notes) {
        let icns = resources.join(format!("{}{ICNS_EXTENSION}", config.binary_name));
        let generated = icons::generate_icns(&source, &icns);
        let written = record_icons(&source, generated, notes);
        if !written.is_empty() {
            generated_icon = icns
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
        }
        artifacts.extend(written);
    }

    let plist = contents.join("Info.plist");
    let project_plist = project_dir.join("macos").join("Info.plist");
    if project_plist.is_file() {
        copy_file(&project_plist, &plist)?;
        // A generated plist is built from `config` by construction and has
        // nothing to reconcile; only the copied one can disagree.
        reconcile(runner, &plist, config, generated_icon.as_deref(), notes)?;
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

/// Compares the identity the *copied* `plist` declares with the identity this
/// run actually assembled, per key:
///
/// - `CFBundleExecutable` naming anything but the binary in `Contents/MacOS`
///   is a hard [`DesktopBuildError::ExecutableIdentityMismatch`] — Launch
///   Services resolves that name and finds nothing, so the `.app` is dead on
///   arrival, and the comparison is exact and deterministic.
/// - `CFBundleIdentifier` and `CFBundleIconFile` drift are
///   [`BundleNote`]s: the bundle still runs, just under an identifier the
///   manifest doesn't know or with an icon nothing wrote.
/// - Any key that cannot be *read* contributes no verdict at all, and the whole
///   attempt yields a single [`BundleNote::PlistUnverified`] naming the first
///   reason. Tool trouble is never a build failure.
///
/// `generated_icon` is the `.icns` file name this run wrote, or `None` when the
/// icon steps were skipped — in which case `CFBundleIconFile` is not even read.
fn reconcile(
    runner: &dyn ProcessRunner,
    plist: &Path,
    config: &DesktopConfig,
    generated_icon: Option<&str>,
    notes: &mut Vec<BundleNote>,
) -> Result<(), DesktopBuildError> {
    let mut unverified: Option<String> = None;
    let mut read = |key: &str| match read_key(runner, plist, key) {
        Ok(value) => Some(value),
        Err(reason) => {
            unverified.get_or_insert(reason);
            None
        }
    };
    let executable = read(KEY_EXECUTABLE);
    let identifier = read(KEY_IDENTIFIER);
    // Read only when there is something to compare against — a project with no
    // usable icon source has a note for that already, and a plist carrying no
    // `CFBundleIconFile` at all must not read as "unverified" because of it.
    let icon = match generated_icon {
        Some(_) => read(KEY_ICON_FILE),
        None => None,
    };

    if let Some(found) = executable
        && found != config.binary_name
    {
        return Err(DesktopBuildError::ExecutableIdentityMismatch {
            expected: config.binary_name.clone(),
            found,
        });
    }
    if let Some(declared) = identifier
        && declared != config.identifier
    {
        notes.push(BundleNote::IdentifierIdentityMismatch {
            plist: declared,
            manifest: config.identifier.clone(),
        });
    }
    if let (Some(declared), Some(generated)) = (icon, generated_icon)
        && icon_stem(&declared) != icon_stem(generated)
    {
        notes.push(BundleNote::IconIdentityMismatch {
            declared_in: ICON_KEY_SOURCE,
            declared,
            generated: generated.to_string(),
        });
    }
    if let Some(reason) = unverified {
        notes.push(BundleNote::PlistUnverified { reason });
    }
    Ok(())
}

/// One `plutil -extract <key> raw <plist>` read, as a value or a one-line
/// reason it couldn't be had. `raw` prints the bare scalar; whether it is
/// newline-terminated has varied across macOS releases, so the first non-empty
/// line is taken rather than the whole trimmed buffer.
fn read_key(runner: &dyn ProcessRunner, plist: &Path, key: &str) -> Result<String, String> {
    let path = plist.to_string_lossy();
    match runner.run(PLUTIL, &["-extract", key, "raw", &path]) {
        Ok(out) if out.success => out
            .stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("`{PLUTIL}` printed no `{key}` value")),
        Ok(out) => Err(format!(
            "`{PLUTIL} -extract {key} raw` exited non-zero{}",
            first_line(&out.stderr)
        )),
        Err(err) => Err(format!("`{PLUTIL}` could not be run ({err})")),
    }
}

/// `": <first non-empty stderr line>"`, or `""` when the tool said nothing —
/// keeps a [`BundleNote::PlistUnverified`] reason to one line however chatty
/// the failure was.
fn first_line(stderr: &str) -> String {
    match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(line) => format!(": {line}"),
        None => String::new(),
    }
}

/// An icon name with its optional `.icns` extension removed, so
/// `CFBundleIconFile` compares equal whichever spelling the project used.
fn icon_stem(name: &str) -> &str {
    name.strip_suffix(ICNS_EXTENSION).unwrap_or(name)
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
    // rather than failing on the existing signature. `--options runtime`
    // always: it enables the Hardened Runtime, which is harmless for an
    // ad-hoc signature and mandatory for a Developer ID one to notarize —
    // there is no case where leaving it off is preferable.
    let mut args: Vec<&str> = vec![
        "--force",
        "--sign",
        identity.as_str(),
        "--options",
        "runtime",
    ];
    if let Some(entitlements) = entitlements.as_deref() {
        args.push("--entitlements");
        args.push(entitlements);
    }
    if is_apple_issued_identity(&identity) {
        args.push("--timestamp");
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

/// Whether `identity` starts with one of [`APPLE_ISSUED_IDENTITY_PREFIXES`] —
/// the only identities `codesign --timestamp` is passed for.
///
/// Shared with [`super::installer`] rather than duplicated there: the
/// `cargo-packager` config's `signingIdentity` decides whether *that* tool's
/// own codesign pass runs, and that pass always passes `--timestamp` (its
/// `codesign/macos.rs::sign` has no conditional), so the offline guarantee
/// above only holds end-to-end if both sides answer this question the same way.
pub(super) fn is_apple_issued_identity(identity: &str) -> bool {
    APPLE_ISSUED_IDENTITY_PREFIXES
        .iter()
        .any(|prefix| identity.starts_with(prefix))
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
    use crate::process::{FakeProcessRunner, Output};

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

    /// The assembled bundle's own plist — the file that ships, and therefore
    /// the one read back.
    const PLIST: &str = "/projects/my_app/dist/macos/My App.app/Contents/Info.plist";

    /// The default project identity every reconciliation test compares
    /// against: binary `my_app`, identifier `dev.f0x.my_app`.
    fn default_config() -> DesktopConfig {
        config("[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n[desktop]\nname = \"My App\"\n")
    }

    fn plutil_key(key: &str) -> String {
        format!("{PLUTIL} -extract {key} raw {PLIST}")
    }

    /// A runner answering every listed `(key, value)` and nothing else — an
    /// unregistered key is a spawn failure, so a test that registers only two
    /// keys proves the third was never read.
    fn plutil_runner(values: &[(&str, &str)]) -> FakeProcessRunner {
        values
            .iter()
            .fold(FakeProcessRunner::new(), |runner, (key, value)| {
                runner.with(
                    plutil_key(key),
                    Output {
                        success: true,
                        stdout: format!("{value}\n"),
                        stderr: String::new(),
                    },
                )
            })
    }

    fn reconcile_notes(
        runner: &FakeProcessRunner,
        config: &DesktopConfig,
        generated_icon: Option<&str>,
    ) -> Result<Vec<BundleNote>, DesktopBuildError> {
        let mut notes = Vec::new();
        reconcile(runner, Path::new(PLIST), config, generated_icon, &mut notes)?;
        Ok(notes)
    }

    #[test]
    fn a_plist_that_agrees_with_the_manifest_records_nothing() {
        let runner = plutil_runner(&[
            (KEY_EXECUTABLE, "my_app"),
            (KEY_IDENTIFIER, "dev.f0x.my_app"),
            (KEY_ICON_FILE, "my_app"),
        ]);
        let notes = reconcile_notes(&runner, &default_config(), Some("my_app.icns")).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// The launch-breaking case, and the only one that fails a build: macOS
    /// resolves `CFBundleExecutable` inside `Contents/MacOS` and finds nothing.
    #[test]
    fn a_plist_naming_another_executable_fails_the_build() {
        let runner = plutil_runner(&[
            (KEY_EXECUTABLE, "renamed_app"),
            (KEY_IDENTIFIER, "dev.f0x.my_app"),
        ]);
        let err = reconcile_notes(&runner, &default_config(), None).unwrap_err();
        assert!(
            matches!(
                &err,
                DesktopBuildError::ExecutableIdentityMismatch { expected, found }
                    if expected == "my_app" && found == "renamed_app"
            ),
            "{err}"
        );
        let message = err.to_string();
        assert!(message.contains("would not launch"), "{message}");
    }

    /// Everything else drifts non-fatally: the bundle still runs, under the
    /// identifier its own plist names.
    #[test]
    fn identifier_and_icon_drift_are_notes_not_failures() {
        let runner = plutil_runner(&[
            (KEY_EXECUTABLE, "my_app"),
            (KEY_IDENTIFIER, "com.example.other"),
            (KEY_ICON_FILE, "AppIcon"),
        ]);
        let notes = reconcile_notes(&runner, &default_config(), Some("my_app.icns")).unwrap();
        assert_eq!(
            notes,
            vec![
                BundleNote::IdentifierIdentityMismatch {
                    plist: "com.example.other".to_string(),
                    manifest: "dev.f0x.my_app".to_string(),
                },
                BundleNote::IconIdentityMismatch {
                    declared_in: ICON_KEY_SOURCE,
                    declared: "AppIcon".to_string(),
                    generated: "my_app.icns".to_string(),
                },
            ]
        );
    }

    /// Apple accepts `CFBundleIconFile` with or without the extension, so both
    /// spellings of the same name agree.
    #[test]
    fn an_icon_file_key_matches_with_or_without_the_icns_extension() {
        for declared in ["my_app", "my_app.icns"] {
            let runner = plutil_runner(&[
                (KEY_EXECUTABLE, "my_app"),
                (KEY_IDENTIFIER, "dev.f0x.my_app"),
                (KEY_ICON_FILE, declared),
            ]);
            let notes = reconcile_notes(&runner, &default_config(), Some("my_app.icns")).unwrap();
            assert!(notes.is_empty(), "{declared}: {notes:?}");
        }
    }

    /// Nothing is compared against a file this run didn't write: with the icon
    /// steps skipped, `CFBundleIconFile` is not even read — proven by the
    /// runner having no registration for it, which would otherwise surface as
    /// an `PlistUnverified` note.
    #[test]
    fn the_icon_key_is_not_read_when_no_icon_was_generated() {
        let runner = plutil_runner(&[
            (KEY_EXECUTABLE, "my_app"),
            (KEY_IDENTIFIER, "dev.f0x.my_app"),
        ]);
        let notes = reconcile_notes(&runner, &default_config(), None).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// Tool trouble never blocks a build: no `plutil` on `PATH` is one note,
    /// not three, and certainly not a failure.
    #[test]
    fn a_missing_plutil_is_one_unverified_note_and_no_verdict() {
        let notes = reconcile_notes(
            &FakeProcessRunner::new(),
            &default_config(),
            Some("my_app.icns"),
        )
        .unwrap();
        assert!(
            matches!(notes.as_slice(), [BundleNote::PlistUnverified { .. }]),
            "{notes:?}"
        );
        assert!(notes[0].to_string().contains("plutil"), "{:?}", notes[0]);
    }

    /// A non-zero exit (a missing key, an unreadable plist) reads the same way
    /// — unverified, with the tool's own first stderr line carried along.
    #[test]
    fn a_failing_plutil_read_is_unverified_rather_than_a_mismatch() {
        let runner = FakeProcessRunner::new().with(
            plutil_key(KEY_EXECUTABLE),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "No value at that key path or invalid key path: CFBundleExecutable\n"
                    .to_string(),
            },
        );
        let notes = reconcile_notes(&runner, &default_config(), None).unwrap();
        assert!(
            matches!(notes.as_slice(), [BundleNote::PlistUnverified { .. }]),
            "{notes:?}"
        );
        assert!(
            notes[0].to_string().contains("invalid key path"),
            "{:?}",
            notes[0]
        );
    }

    /// Output that isn't a value at all (an empty `raw` print) is the third
    /// unverifiable shape, and lands in the same place.
    #[test]
    fn an_empty_plutil_value_is_unverified() {
        let runner = plutil_runner(&[(KEY_EXECUTABLE, "  ")]);
        let notes = reconcile_notes(&runner, &default_config(), None).unwrap();
        assert!(
            matches!(notes.as_slice(), [BundleNote::PlistUnverified { .. }]),
            "{notes:?}"
        );
    }

    /// A key that *can* be read is still judged when another one can't: the
    /// unverified note is per-attempt, not a blanket "skip the whole check".
    #[test]
    fn a_readable_key_is_still_compared_when_another_one_is_unreadable() {
        let runner = plutil_runner(&[(KEY_IDENTIFIER, "com.example.other")]);
        let notes = reconcile_notes(&runner, &default_config(), None).unwrap();
        assert!(
            matches!(
                notes.as_slice(),
                [
                    BundleNote::IdentifierIdentityMismatch { .. },
                    BundleNote::PlistUnverified { .. }
                ]
            ),
            "{notes:?}"
        );
    }
}
