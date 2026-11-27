//! `frust plugin add|list|sync`: the plugins of the project in the current
//! directory. `add` is the shell route to the TUI's Add Plugin dialog.
//!
//! Every subcommand looks plugins up in one [`PluginCatalog`]: the built-in
//! registry plus the plugins the packages of the app's resolved dependency
//! graph describe in their own manifest, assembled by frust-drive
//! ([`project_catalog`], one `cargo metadata` run). This module prints what
//! frust-drive returns and picks the exit code; `frust run` and `frust build`
//! use the same catalog for the machine-local wiring and the pending stop.
//!
//! `add` delegates to [`frust_drive::plugin::add_plugin_from`], so it
//! inherits that function's behaviour unchanged, including resolving packages
//! with `LockfilePolicy::AllowUpdate` (an existing `Cargo.lock` may gain the
//! new dependency). A built-in id needs no discovery — a discovered plugin
//! never shadows a built-in one — so adding one runs no `cargo metadata` of
//! its own, as before.

use std::fmt::Display;
use std::io::Write;
use std::path::Path;

use anyhow::Result;

use frust_drive::packages::CargoLocator;
use frust_drive::plugin::{
    AddOutcome, AddReport, CatalogEntry, DepSource, PluginAddError, PluginCatalog, PluginOrigin,
    add_plugin_from, plugin_statuses, project_catalog, project_catalog_or_built_in, sync_plugins,
};
use frust_drive::process::ProcessRunner;

/// Prints each of `warnings` on its own stderr line.
fn print_warnings(warnings: &[impl Display]) {
    for warning in warnings {
        eprintln!("{warning}");
    }
}

/// How `frust plugin list` names where an entry came from: `built-in`, or
/// `<package> <version> (path|registry|other)`.
pub(crate) fn origin_label(entry: &CatalogEntry) -> String {
    match &entry.origin {
        PluginOrigin::BuiltIn => "built-in".to_string(),
        PluginOrigin::Manifest {
            package,
            version,
            source,
            ..
        } => {
            let source = match source {
                DepSource::Path => "path",
                DepSource::Registry => "registry",
                DepSource::Other => "other",
            };
            format!("{package} {version} ({source})")
        }
    }
}

/// `frust plugin list` for the project rooted at `project_root`: discovers
/// its catalog through `runner` and prints it ([`list_with`]).
pub fn list(runner: &dyn ProcessRunner, project_root: &Path) -> Result<u8> {
    let found = project_catalog_or_built_in(&CargoLocator::new(runner), project_root);
    print_warnings(&found.warnings);
    list_with(
        &found.catalog,
        project_root,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

/// The testable core of [`list`]: one line per plugin of `catalog`, in the
/// order and with the status [`plugin_statuses`] gives, and its origin. Exit
/// code 1 when the app's `Cargo.toml` cannot be read.
pub(crate) fn list_with(
    catalog: &PluginCatalog,
    project_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let statuses = match plugin_statuses(project_root, catalog) {
        Ok(statuses) => statuses,
        Err(error) => {
            writeln!(err, "Error: {error}")?;
            return Ok(1);
        }
    };
    let rows: Vec<(&str, String, String)> = statuses
        .iter()
        .map(|(entry, status)| (&*entry.spec.id, origin_label(entry), status.to_string()))
        .collect();
    let id_width = rows.iter().map(|(id, ..)| id.len()).max().unwrap_or(0);
    let origin_width = rows
        .iter()
        .map(|(_, origin, _)| origin.len())
        .max()
        .unwrap_or(0);
    for (id, origin, status) in rows {
        let line = format!("{id:<id_width$}  {origin:<origin_width$}  {status}");
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(0)
}

/// `frust plugin sync` for the project rooted at `project_root`: discovers
/// its catalog through `runner` and applies it ([`sync_with`]). A failed
/// discovery fails the command, since there is nothing to sync without it.
pub fn sync(runner: &dyn ProcessRunner, project_root: &Path) -> Result<u8> {
    let mut err = std::io::stderr();
    let catalog = match project_catalog(&CargoLocator::new(runner), project_root) {
        Ok(found) => {
            print_warnings(&found.warnings);
            found.catalog
        }
        Err(error) => {
            writeln!(
                err,
                "Error: could not discover the plugins this project's dependencies describe: \
                 {error}"
            )?;
            return Ok(1);
        }
    };
    sync_with(&catalog, project_root, &mut std::io::stdout(), &mut err)
}

/// The testable core of [`sync`]: [`sync_plugins`] over `catalog`, printing
/// per plugin what was applied, or `up to date`. Exit code 0 when nothing
/// failed (a re-run is a no-op), 1 on a [`PluginAddError`], with nothing
/// written.
pub(crate) fn sync_with(
    catalog: &PluginCatalog,
    project_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let report = match sync_plugins(project_root, catalog) {
        Ok(report) => report,
        Err(error) => {
            writeln!(err, "Error: {error}")?;
            return Ok(1);
        }
    };
    if report.plugins.is_empty() {
        writeln!(
            out,
            "No dependency of this project describes a plugin in its package manifest; \
             nothing to sync."
        )?;
    }
    for plugin in &report.plugins {
        let applied: Vec<&str> = plugin
            .items
            .iter()
            .filter(|item| item.outcome == AddOutcome::Applied)
            .map(|item| item.description.as_str())
            .collect();
        if applied.is_empty() {
            writeln!(out, "Plugin `{}`: up to date", plugin.plugin_id)?;
        } else {
            writeln!(
                out,
                "Plugin `{}`: {} applied",
                plugin.plugin_id,
                applied.len()
            )?;
            for description in applied {
                writeln!(out, "  [applied] {description}")?;
            }
        }
    }
    Ok(0)
}

/// Adds plugin `id` with `features` to the project rooted at `project_root`
/// (no walk-up), printing the report on stdout or the error on stderr. An id
/// the built-in registry lacks is looked up in the project's catalog,
/// discovered through `runner`. Returns the process exit code: 0 on success
/// (including a no-op re-run), 1 on a [`PluginAddError`].
pub fn add(
    runner: &dyn ProcessRunner,
    project_root: &Path,
    id: &str,
    features: &[String],
) -> Result<u8> {
    let built_in = PluginCatalog::built_in();
    let catalog = if built_in.find(id).is_some() {
        built_in
    } else {
        let found = project_catalog_or_built_in(&CargoLocator::new(runner), project_root);
        print_warnings(&found.warnings);
        found.catalog
    };
    run_with(
        |root, id, features| add_plugin_from(root, &catalog, id, features),
        &catalog,
        project_root,
        id,
        features,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

/// The testable core of [`add`], with the apply function and output sinks
/// injected. `catalog` names the valid plugins and features an unknown id or
/// feature is answered with.
pub(crate) fn run_with(
    apply: impl FnOnce(&Path, &str, &[&str]) -> Result<AddReport, PluginAddError>,
    catalog: &PluginCatalog,
    project_root: &Path,
    id: &str,
    features: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    match apply(project_root, id, &feature_refs) {
        Ok(report) => {
            let (applied, already, at_build) = report.outcome_counts();
            writeln!(
                out,
                "Plugin `{}`: {applied} applied, {already} already present, {at_build} applied at build",
                report.plugin_id
            )?;
            for item in &report.items {
                let label = match item.outcome {
                    AddOutcome::Applied => "applied",
                    AddOutcome::AlreadyPresent => "already present",
                    AddOutcome::AppliedAtBuild => "applied at build",
                };
                writeln!(out, "  [{label}] {}", item.description)?;
            }
            Ok(0)
        }
        Err(error) => {
            writeln!(err, "Error: {error}")?;
            match &error {
                PluginAddError::UnknownPlugin(_) => {
                    let ids: Vec<&str> = catalog.specs().map(|p| &*p.id).collect();
                    writeln!(err, "Valid plugins: {}", ids.join(", "))?;
                }
                PluginAddError::UnknownFeature { plugin, .. } => {
                    let valid: Vec<&str> = catalog
                        .find(plugin)
                        .map(|e| e.spec.optional_features.iter().map(|f| &*f.id).collect())
                        .unwrap_or_default();
                    if valid.is_empty() {
                        writeln!(err, "Plugin `{plugin}` has no optional features.")?;
                    } else {
                        writeln!(err, "Valid features for `{plugin}`: {}", valid.join(", "))?;
                    }
                }
                _ => {}
            }
            Ok(1)
        }
    }
}

/// A scaffolded app depending on a plugin package that describes itself in
/// its own manifest, and the `cargo metadata` document a fake runner answers
/// for it — shared by this module's tests and `run`'s and `build`'s.
#[cfg(test)]
pub(crate) mod fixture {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use anyhow::Result;
    use frust_drive::packages::CargoLocator;
    use frust_drive::platform_wiring;
    use frust_drive::plugin::PluginCatalog;
    use frust_drive::process::{FakeProcessRunner, Output, ProcessRunner, StreamHandle};
    use frust_drive::scaffold::{self, FrustDependency, TemplateContext};

    /// The plugin package's name.
    pub(crate) const PACKAGE: &str = "frust-acme-ble";
    /// The plugin's id.
    pub(crate) const ID: &str = "acme-ble";
    /// The plugin's Gradle module and Swift package.
    pub(crate) const GRADLE_MODULE: &str = ":frust-acme-ble";
    pub(crate) const SWIFT_PACKAGE: &str = "FrustAcmeBle";
    /// The project files `frust run`/`frust build` must never edit.
    pub(crate) const TRACKED: [&str; 6] = [
        "Cargo.toml",
        "android/settings.gradle.kts",
        "android/app/build.gradle.kts",
        "android/app/src/main/AndroidManifest.xml",
        "ios/Runner/Info.plist",
        "ios/Runner.xcodeproj/project.pbxproj",
    ];

    /// The plugin's `[package.metadata]`, as `cargo metadata` prints it: a
    /// Gradle module, a permission, a Swift package and a plist key.
    const PLUGIN_METADATA: &str = r#"{"frust":{"plugin":{"id":"acme-ble","summary":"Acme BLE scanner","android":{"gradle-module":{"name":"frust-acme-ble","path":"platform/android"},"permissions":["android.permission.BLUETOOTH_CONNECT"]},"ios":{"swift-package":{"name":"FrustAcmeBle","path":"platform/ios"},"plist":{"NSBluetoothAlwaysUsageDescription":"Finds nearby Acme devices."}}}}}"#;

    /// The app at `<root>/app`, the plugin package at `<root>/acme-ble` and
    /// the two shell crates under `<root>/shells`; removed on drop.
    pub(crate) struct Fixture {
        pub(crate) root: PathBuf,
        pub(crate) app: PathBuf,
    }

    impl Fixture {
        pub(crate) fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "frust-cli-plugin-fixture-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let app = root.join("app");
            let ctx = TemplateContext {
                project_name: "acme_app".into(),
                title_case_name: "Acme App".into(),
                org: "com.example".into(),
                description: "A test app.".into(),
                frust_version: "0.1.0".into(),
                frust: FrustDependency::Registry {
                    version: "0.1.0".into(),
                },
                deeplink_scheme: None,
                deeplink_host: None,
            };
            scaffold::generate(&app, &ctx, None, false, None).unwrap();
            for dir in [
                "acme-ble/platform/android",
                "acme-ble/platform/ios",
                "shells/frust-shell-android/platform/android/frust-embedding",
                "shells/frust-shell-ios/platform/ios/FrustEmbedding",
            ] {
                std::fs::create_dir_all(root.join(dir)).unwrap();
            }
            let fixture = Self { root, app };
            fixture.depend(&format!("{PACKAGE} = {{ path = \"../acme-ble\" }}"));
            fixture
        }

        /// `line` added to the app's `[dependencies]`.
        pub(crate) fn depend(&self, line: &str) {
            let path = self.app.join("Cargo.toml");
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(
                &path,
                text.replacen("[dependencies]\n", &format!("[dependencies]\n{line}\n"), 1),
            )
            .unwrap();
        }

        /// The `cargo metadata` invocation the app's graph is read with.
        pub(crate) fn metadata_key(&self) -> String {
            format!(
                "cargo metadata --format-version 1 --manifest-path {}",
                self.app.join("Cargo.toml").display()
            )
        }

        /// The app's resolved graph: the app, both shells and the plugin.
        pub(crate) fn metadata(&self) -> String {
            let package = |name: &str, version: &str, dir: &Path, metadata: &str| {
                let manifest = dir
                    .join("Cargo.toml")
                    .display()
                    .to_string()
                    .replace('\\', "\\\\");
                format!(
                    r#"{{"name":"{name}","version":"{version}","id":"{name}@{version}","source":null,"manifest_path":"{manifest}","features":{{}},"metadata":{metadata},"dependencies":[]}}"#
                )
            };
            let packages = [
                package("acme_app", "0.1.0", &self.app, "null"),
                package(
                    platform_wiring::ANDROID_SHELL_PACKAGE,
                    "0.1.0",
                    &self.root.join("shells/frust-shell-android"),
                    "null",
                ),
                package(
                    platform_wiring::IOS_SHELL_PACKAGE,
                    "0.1.0",
                    &self.root.join("shells/frust-shell-ios"),
                    "null",
                ),
                package(
                    PACKAGE,
                    "0.3.1",
                    &self.root.join("acme-ble"),
                    PLUGIN_METADATA,
                ),
            ];
            format!(
                r#"{{"packages":[{}],"workspace_members":["acme_app@0.1.0"],"resolve":null,"version":1}}"#,
                packages.join(",")
            )
        }

        /// A runner answering [`Self::metadata_key`], recording every call.
        pub(crate) fn runner(&self) -> Recording {
            Recording::new(FakeProcessRunner::new().with(
                self.metadata_key(),
                Output {
                    success: true,
                    stdout: self.metadata(),
                    stderr: String::new(),
                },
            ))
        }

        /// The app's catalog, discovered through a fresh [`Self::runner`].
        pub(crate) fn catalog(&self) -> PluginCatalog {
            let runner = self.runner();
            let found = super::project_catalog(&CargoLocator::new(&runner), &self.app).unwrap();
            if let Some(warning) = found.warnings.first() {
                panic!("unexpected warning: {warning}")
            }
            found.catalog
        }

        /// The bytes of every [`TRACKED`] file.
        pub(crate) fn tracked(&self) -> Vec<Vec<u8>> {
            TRACKED
                .iter()
                .map(|rel| std::fs::read(self.app.join(rel)).unwrap())
                .collect()
        }

        /// Whether the plugin's machine-local wiring is in place: its
        /// `local.properties` key and its `ios/` link.
        pub(crate) fn wired(&self) -> (bool, bool) {
            let key = platform_wiring::plugin_module_key(GRADLE_MODULE);
            let properties =
                std::fs::read_to_string(self.app.join(platform_wiring::LOCAL_PROPERTIES))
                    .unwrap_or_default();
            let link = self
                .app
                .join(platform_wiring::swift_package_link(SWIFT_PACKAGE));
            (
                properties
                    .lines()
                    .any(|line| line.starts_with(&format!("{key}="))),
                std::fs::read_link(&link).ok() == Some(self.root.join("acme-ble/platform/ios")),
            )
        }

        /// Removes the plugin's machine-local wiring.
        pub(crate) fn unwire(&self) {
            let _ = std::fs::remove_file(self.app.join(platform_wiring::LOCAL_PROPERTIES));
            let _ = std::fs::remove_file(
                self.app
                    .join(platform_wiring::swift_package_link(SWIFT_PACKAGE)),
            );
            assert_eq!(self.wired(), (false, false));
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A [`FakeProcessRunner`] that records every invocation it is asked for.
    pub(crate) struct Recording {
        inner: FakeProcessRunner,
        calls: Mutex<Vec<String>>,
    }

    impl Recording {
        fn new(inner: FakeProcessRunner) -> Self {
            Self {
                inner,
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Every invocation so far, as `<cmd> <args...>`.
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn record(&self, cmd: &str, args: &[&str]) {
            let call = std::iter::once(cmd)
                .chain(args.iter().copied())
                .collect::<Vec<_>>()
                .join(" ");
            self.calls.lock().unwrap().push(call);
        }
    }

    impl ProcessRunner for Recording {
        fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
            self.record(cmd, args);
            self.inner.run(cmd, args)
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> Result<Output> {
            self.record(cmd, args);
            self.inner.run_streaming(cmd, args, cwd, env, on_line)
        }

        fn run_streaming_scrubbed(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            remove_env: &[&str],
            on_line: &mut dyn FnMut(&str),
        ) -> Result<Output> {
            self.record(cmd, args);
            self.inner
                .run_streaming_scrubbed(cmd, args, cwd, env, remove_env, on_line)
        }

        fn run_streaming_with_stderr(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
            on_stderr_line: &mut dyn FnMut(&str),
        ) -> Result<Output> {
            self.record(cmd, args);
            self.inner
                .run_streaming_with_stderr(cmd, args, cwd, env, on_line, on_stderr_line)
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
        ) -> Result<StreamHandle> {
            self.record(cmd, args);
            self.inner.spawn_streaming(cmd, args, cwd, env)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::packages::StubLocator;
    use frust_drive::plugin::{FloorPlatform, add_plugin_with};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn project(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-plugin-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfrust = { package = \"frust-ui\", version = \"0.6\" }\n",
        )
        .unwrap();
        dir
    }

    fn run(dir: &Path, id: &str, features: &[&str]) -> (u8, String, String) {
        let features: Vec<String> = features.iter().map(|f| f.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            |root, id, feats| add_plugin_with(&StubLocator::new(), root, id, feats),
            &PluginCatalog::built_in(),
            dir,
            id,
            &features,
            &mut out,
            &mut err,
        )
        .unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn success_then_idempotent_rerun() {
        let dir = project("ok");
        let (code, out, _) = run(&dir, "device-info", &[]);
        assert_eq!(code, 0);
        assert!(out.contains("[applied]"), "{out}");
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("frust-device-info"));

        let (code, out, _) = run(&dir, "device-info", &[]);
        assert_eq!(code, 0);
        assert!(out.contains("0 applied"), "{out}");
        assert!(!out.contains("[applied]"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("Cargo.toml")).unwrap(),
            manifest
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_id_lists_valid_plugins() {
        let dir = project("badid");
        let (code, _, err) = run(&dir, "nope", &[]);
        assert_eq!(code, 1);
        assert!(err.contains("unknown plugin `nope`"), "{err}");
        assert!(err.contains("device-info"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_feature_lists_valid_features() {
        let dir = project("badfeat");
        let (code, _, err) = run(&dir, "network-image", &["nope"]);
        assert_eq!(code, 1);
        assert!(
            err.contains("Valid features for `network-image`: webp"),
            "{err}"
        );
        let (code, _, err) = run(&dir, "device-info", &["nope"]);
        assert_eq!(code, 1);
        assert!(err.contains("no optional features"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A platform floor the app is below is printed as it reads and fails the
    /// command, with nothing written (the refusal happens before any edit).
    #[test]
    fn a_platform_floor_refusal_is_printed_and_exits_non_zero() {
        let dir = project("floor");
        let before = std::fs::read(dir.join("Cargo.toml")).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            |_, _, _| {
                Err(PluginAddError::PlatformFloorTooLow {
                    plugin: "floor-plugin".into(),
                    platform: FloorPlatform::Ios,
                    required: "16.4".into(),
                    found: "15.0".into(),
                    key: "IPHONEOS_DEPLOYMENT_TARGET",
                    file: "ios/Runner.xcodeproj/project.pbxproj",
                    hint: "Raise it in every build configuration, then re-run.",
                })
            },
            &PluginCatalog::built_in(),
            &dir,
            "floor-plugin",
            &[],
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "Error: plugin `floor-plugin` needs iOS 16.4 or later; \
             ios/Runner.xcodeproj/project.pbxproj sets IPHONEOS_DEPLOYMENT_TARGET = 15.0. \
             Raise it in every build configuration, then re-run.\n"
        );
        assert_eq!(std::fs::read(dir.join("Cargo.toml")).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outside_a_project_fails() {
        let dir =
            std::env::temp_dir().join(format!("frust-cli-plugin-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _, err) = run(&dir, "device-info", &[]);
        assert_eq!(code, 1);
        assert!(err.contains("Cargo.toml"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The subcommands over a catalog with a discovered plugin ([`fixture`]).
#[cfg(test)]
mod discovered_tests {
    use super::fixture::{Fixture, ID, PACKAGE};
    use super::*;
    use frust_drive::packages::StubLocator;
    use frust_drive::plugin::{
        add_plugin_from_with, add_plugin_with, installed_plugin_ids, pending_contributions,
    };
    use frust_drive::process::{FakeProcessRunner, Output};

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap()
    }

    /// The message a native build stops with ([`frust_drive::plugin::pending_stop`]),
    /// `None` when nothing is pending; any warning fails the test.
    fn pending_stop(app: &Path, catalog: &PluginCatalog) -> Option<String> {
        let stop = frust_drive::plugin::pending_stop(app, catalog);
        if let Some(warning) = stop.warnings.first() {
            panic!("unexpected warning: {warning}")
        }
        stop.stops().then(|| stop.to_string())
    }

    fn list(fx: &Fixture, catalog: &PluginCatalog) -> String {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(list_with(catalog, &fx.app, &mut out, &mut err).unwrap(), 0);
        assert_eq!(text(err), "");
        text(out)
    }

    fn sync(fx: &Fixture, catalog: &PluginCatalog) -> String {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(sync_with(catalog, &fx.app, &mut out, &mut err).unwrap(), 0);
        assert_eq!(text(err), "");
        text(out)
    }

    fn add(fx: &Fixture, catalog: &PluginCatalog, id: &str) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let locator = StubLocator::failing("a manifest plugin is never located");
        let code = run_with(
            |root, id, features| add_plugin_from_with(&locator, root, catalog, id, features),
            catalog,
            &fx.app,
            id,
            &[],
            &mut out,
            &mut err,
        )
        .unwrap();
        (code, text(out), text(err))
    }

    fn line<'a>(listing: &'a str, id: &str) -> &'a str {
        listing
            .lines()
            .find(|line| line.split_whitespace().next() == Some(id))
            .unwrap_or_else(|| panic!("no `{id}` line in:\n{listing}"))
    }

    #[test]
    fn list_shows_each_plugin_with_its_origin_and_status() {
        let fx = Fixture::new("list");
        add_plugin_with(&StubLocator::new(), &fx.app, "device-info", &[]).unwrap();
        let catalog = fx.catalog();
        let listing = list(&fx, &catalog);
        let lines: Vec<&str> = listing.lines().collect();
        assert_eq!(lines.len(), catalog.iter().count(), "{listing}");

        let device_info = line(&listing, "device-info");
        assert!(device_info.contains(" built-in "), "{device_info}");
        assert!(device_info.ends_with(" added"), "{device_info}");
        // A built-in the app does not depend on carries no status.
        assert!(line(&listing, "camera").ends_with(" built-in"), "{listing}");
        // The discovered plugin comes after every built-in one.
        let acme = *lines.last().unwrap();
        assert_eq!(acme, line(&listing, ID));
        assert!(acme.contains(" frust-acme-ble 0.3.1 (path) "), "{acme}");
        assert!(acme.ends_with(" 4 pending"), "{acme}");

        sync(&fx, &catalog);
        assert!(line(&list(&fx, &catalog), ID).ends_with(" added"));
    }

    #[test]
    fn list_outside_a_project_fails() {
        let fx = Fixture::new("list-outside");
        std::fs::remove_file(fx.app.join("Cargo.toml")).unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = list_with(&PluginCatalog::built_in(), &fx.app, &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert!(text(err).contains("Cargo.toml"));
        assert!(out.is_empty());
    }

    #[test]
    fn add_resolves_a_discovered_id_and_a_rerun_is_a_no_op() {
        let fx = Fixture::new("add");
        let catalog = fx.catalog();
        let (code, out, err) = add(&fx, &catalog, ID);
        assert_eq!(code, 0, "{err}");
        assert!(out.starts_with("Plugin `acme-ble`: 4 applied"), "{out}");
        let entry = catalog.find(ID).unwrap();
        assert_eq!(pending_contributions(&fx.app, entry, &[]).unwrap(), []);

        let tracked = fx.tracked();
        let (code, out, _) = add(&fx, &catalog, ID);
        assert_eq!(code, 0);
        assert!(out.starts_with("Plugin `acme-ble`: 0 applied"), "{out}");
        assert!(!out.contains("[applied]"), "{out}");
        assert_eq!(fx.tracked(), tracked);
    }

    #[test]
    fn an_unknown_id_lists_built_in_and_discovered_ids() {
        let fx = Fixture::new("add-unknown");
        let (code, _, err) = add(&fx, &fx.catalog(), "nope");
        assert_eq!(code, 1);
        assert!(err.contains("unknown plugin `nope`"), "{err}");
        let valid = err
            .lines()
            .find(|l| l.starts_with("Valid plugins: "))
            .unwrap();
        assert!(valid.contains("device-info"), "{valid}");
        assert!(valid.ends_with(", acme-ble"), "{valid}");
    }

    #[test]
    fn sync_applies_pending_items_then_reports_up_to_date() {
        let fx = Fixture::new("sync");
        let catalog = fx.catalog();
        let out = sync(&fx, &catalog);
        assert!(out.starts_with("Plugin `acme-ble`: 4 applied\n"), "{out}");
        assert!(
            out.contains(
                "  [applied] AndroidManifest.xml permission `android.permission.BLUETOOTH_CONNECT`\n"
            ),
            "{out}"
        );
        assert_eq!(fx.wired(), (true, true));

        let tracked = fx.tracked();
        assert_eq!(sync(&fx, &catalog), "Plugin `acme-ble`: up to date\n");
        assert_eq!(fx.tracked(), tracked);
    }

    /// A Swift package the scaffolded `/ios/Frust*` line does not cover adds
    /// a `.gitignore` row: the build's pending stop and the sync's applied
    /// count name the same rows.
    #[test]
    fn sync_counts_the_rows_the_pending_stop_lists() {
        let fx = Fixture::new("sync-count");
        let runner = FakeProcessRunner::new().with(
            fx.metadata_key(),
            Output {
                success: true,
                stdout: fx.metadata().replace("FrustAcmeBle", "AcmeLink"),
                stderr: String::new(),
            },
        );
        let catalog = project_catalog(&CargoLocator::new(&runner), &fx.app)
            .unwrap()
            .catalog;
        let stop = pending_stop(&fx.app, &catalog).unwrap();
        let pending: Vec<&str> = stop
            .lines()
            .filter_map(|line| line.strip_prefix("  - "))
            .collect();
        assert_eq!(pending.len(), 5, "{stop}");
        assert!(stop.contains(": 5 change(s) pending."), "{stop}");

        let out = sync(&fx, &catalog);
        assert!(out.starts_with("Plugin `acme-ble`: 5 applied\n"), "{out}");
        let applied: Vec<&str> = out
            .lines()
            .filter_map(|line| line.strip_prefix("  [applied] "))
            .collect();
        assert_eq!(applied, pending);
        assert!(
            applied.contains(&".gitignore entry `/ios/AcmeLink`"),
            "{out}"
        );
        assert_eq!(pending_stop(&fx.app, &catalog), None);
    }

    #[test]
    fn sync_without_a_discovered_dependency_has_nothing_to_do() {
        let fx = Fixture::new("sync-none");
        let out = sync(&fx, &PluginCatalog::built_in());
        assert_eq!(
            out,
            "No dependency of this project describes a plugin in its package manifest; \
             nothing to sync.\n"
        );
    }

    #[test]
    fn a_refused_description_and_a_shadowed_id_are_warnings() {
        let fx = Fixture::new("warnings");
        let extra = [
            ("frust-acme-cam", r#"{"frust":{"plugin":{"id":"camera","summary":"Acme camera"}}}"#),
            ("frust-acme-bad", r#"{"frust":{"plugin":{"id":"Acme_Bad","summary":"Acme"}}}"#),
        ]
        .map(|(name, metadata)| {
            let manifest = fx.root.join(name).join("Cargo.toml");
            format!(
                r#"{{"name":"{name}","version":"0.1.0","id":"{name}@0.1.0","source":null,"manifest_path":"{}","features":{{}},"metadata":{metadata},"dependencies":[]}}"#,
                manifest.display().to_string().replace('\\', "\\\\")
            )
        });
        let metadata = fx.metadata().replacen(
            "],\"workspace_members\"",
            &format!(",{}],\"workspace_members\"", extra.join(",")),
            1,
        );
        let runner = FakeProcessRunner::new().with(
            fx.metadata_key(),
            Output {
                success: true,
                stdout: metadata,
                stderr: String::new(),
            },
        );
        let found = project_catalog(&CargoLocator::new(&runner), &fx.app).unwrap();
        let warnings: Vec<String> = found.warnings.iter().map(ToString::to_string).collect();
        let catalog = found.catalog;
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].starts_with("warning: plugin manifest of package `frust-acme-bad`: "),
            "{warnings:?}"
        );
        assert_eq!(
            warnings[1],
            "warning: plugin `camera` described by package `frust-acme-cam` is ignored: a \
             built-in plugin has the same id"
        );
        assert!(catalog.find("camera").unwrap().is_built_in());
        assert!(catalog.find(ID).is_some());
    }

    #[test]
    fn a_failed_discovery_falls_back_to_the_built_in_plugins_with_a_warning() {
        let fx = Fixture::new("discovery-fails");
        let runner = FakeProcessRunner::new().with(
            fx.metadata_key(),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: no matching package named `frust-ui` found".to_string(),
            },
        );
        let found = project_catalog_or_built_in(&CargoLocator::new(&runner), &fx.app);
        let warnings: Vec<String> = found.warnings.iter().map(ToString::to_string).collect();
        let catalog = found.catalog;
        assert_eq!(catalog, PluginCatalog::built_in());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("no matching package"), "{warnings:?}");
    }

    #[test]
    fn the_pending_stop_names_the_plugin_and_lists_every_item() {
        let fx = Fixture::new("stop");
        let catalog = fx.catalog();
        let message = pending_stop(&fx.app, &catalog).unwrap();
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(
            lines,
            [
                "plugin acme-ble (frust-acme-ble) is not wired into this project: 4 change(s) \
                 pending. Run `frust plugin sync` and re-run.",
                "  - Gradle module `:frust-acme-ble`",
                "  - AndroidManifest.xml permission `android.permission.BLUETOOTH_CONNECT`",
                "  - Xcode Swift package `FrustAcmeBle`",
                "  - Info.plist key `NSBluetoothAlwaysUsageDescription`",
            ]
        );
        sync(&fx, &catalog);
        assert_eq!(pending_stop(&fx.app, &catalog), None);
    }

    /// Only a plugin `frust plugin sync` would apply can stop a build: one the
    /// app does not depend on, or depends on optionally, never does.
    #[test]
    fn the_pending_stop_ignores_a_plugin_the_app_does_not_depend_on() {
        let fx = Fixture::new("stop-optional");
        let catalog = fx.catalog();
        let manifest = fx.app.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let line = format!("{PACKAGE} = {{ path = \"../acme-ble\" }}\n");
        let optional = format!("{PACKAGE} = {{ path = \"../acme-ble\", optional = true }}\n");
        for replacement in [optional.as_str(), ""] {
            std::fs::write(&manifest, text.replacen(&line, replacement, 1)).unwrap();
            let installed = installed_plugin_ids(&fx.app, &catalog).unwrap();
            assert!(!installed.contains(ID), "{installed:?}");
            assert_eq!(pending_stop(&fx.app, &catalog), None);
            let mut out = Vec::new();
            assert_eq!(
                sync_with(&catalog, &fx.app, &mut out, &mut Vec::new()).unwrap(),
                0
            );
            assert!(String::from_utf8(out).unwrap().contains("nothing to sync"));
        }
    }
}
