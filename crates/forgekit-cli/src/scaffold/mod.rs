//! Data-driven project scaffolding (spec §12.3), modeled on Flutter's
//! `templates/` mechanism: a manifest-listed file tree rendered against a
//! [`TemplateContext`].

pub mod context;
pub mod renderer;

#[allow(unused_imports)]
// NameError: public API surface for future callers matching on variants
pub use context::{NameError, TemplateContext, title_case, validate_project_name};

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use include_dir::{Dir, include_dir};

/// The `templates/app/` tree, embedded into the `forgekit` binary at
/// compile time so `forgekit create` works standalone without a repo
/// checkout at runtime.
static EMBEDDED_APP_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../templates/app");

/// Name of the manifest file (relative to the template root) that
/// whitelists every file the template ships. Never itself copied into a
/// generated project.
const MANIFEST_FILE: &str = "template_manifest.json";

/// How a manifest entry's content should be handled (spec §12.3 extension
/// conventions).
enum FileMode {
    /// `<file>.tmpl` → minijinja-render, strip `.tmpl`.
    Render,
    /// `<file>.copy.tmpl` → copy verbatim, strip `.copy.tmpl`.
    CopyVerbatim,
    /// Anything else in the manifest → copy as-is.
    CopyAsIs,
}

/// Splits a manifest entry into its content-handling mode and the logical
/// (post-suffix-stripping) relative path.
fn classify(entry: &str) -> (FileMode, &str) {
    if let Some(stem) = entry.strip_suffix(".copy.tmpl") {
        (FileMode::CopyVerbatim, stem)
    } else if let Some(stem) = entry.strip_suffix(".tmpl") {
        (FileMode::Render, stem)
    } else {
        (FileMode::CopyAsIs, entry)
    }
}

/// Where to read the template tree from: the binary's embedded copy, or a
/// filesystem override (`--template-dir`, undocumented, development only).
enum Source<'a> {
    Embedded,
    Dir(&'a Path),
}

impl Source<'_> {
    fn manifest(&self) -> Result<Vec<String>> {
        let raw = self.read_to_string(MANIFEST_FILE)?;
        serde_json::from_str(&raw).context("parsing template_manifest.json")
    }

    fn read(&self, relative: &str) -> Result<Vec<u8>> {
        match self {
            Source::Embedded => EMBEDDED_APP_TEMPLATE
                .get_file(relative)
                .map(|f| f.contents().to_vec())
                .ok_or_else(|| {
                    anyhow!(
                        "template file `{relative}` missing from embedded template (manifest drift)"
                    )
                }),
            Source::Dir(root) => fs::read(root.join(relative))
                .with_context(|| format!("reading template file `{relative}`")),
        }
    }

    fn read_to_string(&self, relative: &str) -> Result<String> {
        let bytes = self.read(relative)?;
        String::from_utf8(bytes)
            .with_context(|| format!("template file `{relative}` is not valid UTF-8"))
    }
}

/// Generates a new app at `dest` from the embedded `templates/app` tree (or
/// `template_dir_override`, for development). Returns the relative paths
/// written, in manifest order. Refuses a non-empty `dest` unless
/// `overwrite` is set.
pub fn generate(
    dest: &Path,
    ctx: &TemplateContext,
    template_dir_override: Option<&Path>,
    overwrite: bool,
) -> Result<Vec<PathBuf>> {
    check_destination(dest, overwrite)?;

    // Fail fast (spec Phase 3 task 34): a grammatically invalid derived iOS
    // bundle identifier is rejected before any file is written, rather than
    // emitting an Xcode project that `xcodebuild` would later refuse.
    ctx.validate_ios_identifier().map_err(|err| {
        anyhow!("invalid iOS bundle identifier derived from `org` + project name: {err}")
    })?;

    let source = match template_dir_override {
        Some(dir) => Source::Dir(dir),
        None => Source::Embedded,
    };
    let manifest = source.manifest()?;
    let render_vars = ctx.render_vars();
    let path_vars = ctx.path_vars();

    let mut written = Vec::with_capacity(manifest.len());
    for entry in &manifest {
        let (mode, logical) = classify(entry);
        // A source-tree directory (e.g. `android.tmpl/`) may itself carry
        // a `.tmpl` suffix as a purely organizational marker; strip it
        // before path-placeholder expansion so it doesn't leak into the
        // generated project (`android.tmpl/` → `android/`).
        let logical = renderer::strip_tmpl_dir_suffixes(Path::new(logical));
        let out_relative = renderer::expand_path(&logical, &path_vars);
        let out_path = dest.join(&out_relative);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory `{}`", parent.display()))?;
        }
        match mode {
            FileMode::Render => {
                let raw = source.read_to_string(entry)?;
                let rendered = renderer::render(&raw, &render_vars)
                    .with_context(|| format!("rendering `{entry}`"))?;
                fs::write(&out_path, rendered)
                    .with_context(|| format!("writing `{}`", out_path.display()))?;
            }
            FileMode::CopyVerbatim | FileMode::CopyAsIs => {
                let bytes = source.read(entry)?;
                fs::write(&out_path, bytes)
                    .with_context(|| format!("writing `{}`", out_path.display()))?;
            }
        }
        preserve_executable_bit(&out_path)
            .with_context(|| format!("setting permissions on `{}`", out_path.display()))?;
        written.push(out_relative);
    }
    Ok(written)
}

/// Filenames the scaffold vendors verbatim that must retain their
/// executable bit in the generated project. `fs::write` always creates
/// files with the umask-default (non-executable) mode — `include_dir` has
/// no permission metadata to read back from the embedded copy, so
/// "preserve the original mode" isn't available and this has to be a
/// filename allowlist instead. Currently only the Gradle wrapper script
/// (spec Phase 2 task 22, `android.tmpl/gradlew`); its Windows counterpart
/// (`gradlew.bat`) doesn't need a Unix exec bit.
const EXECUTABLE_FILENAMES: &[&str] = &["gradlew"];

/// Sets the Unix executable bit (`0o755`) on `path` if its file name is in
/// [`EXECUTABLE_FILENAMES`]. No-op on non-Unix targets and for every other
/// file (matches the mode `fs::write` already produced, so this never
/// *removes* permissions).
fn preserve_executable_bit(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let is_executable = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| EXECUTABLE_FILENAMES.contains(&name));
        if is_executable {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .context("chmod +x on vendored executable script")?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Refuses a non-empty `dest` unless `overwrite` is set, listing the
/// offending files in the error.
fn check_destination(dest: &Path, overwrite: bool) -> Result<()> {
    if overwrite || !dest.exists() {
        return Ok(());
    }
    let entries: Vec<String> = fs::read_dir(dest)
        .with_context(|| format!("reading `{}`", dest.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    bail!(
        "target directory `{}` is not empty (pass --overwrite to proceed):\n  {}",
        dest.display(),
        entries.join("\n  ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new ForgeKit application.".into(),
            forgekit_version: "0.1.0".into(),
            forgekit_path: "/path/to/forgekit".into(),
        }
    }

    #[test]
    fn generate_produces_manifest_listed_files_with_substitutions() {
        let dest = unique_temp_dir("manifest-set");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false).unwrap();
        assert!(!written.is_empty());

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("my_app"), "{cargo_toml}");
        // The app crate builds the Android `cdylib` (JNI bridge), the iOS
        // `staticlib` (C-ABI bridge, task 33), and an `rlib` for the desktop
        // preview binary (task 24).
        assert!(
            cargo_toml.contains("crate-type = [\"cdylib\", \"staticlib\", \"rlib\"]"),
            "{cargo_toml}"
        );
        let forgekit_toml = fs::read_to_string(dest.join("forgekit.toml")).unwrap();
        assert!(forgekit_toml.contains("dev.f0x"), "{forgekit_toml}");

        assert!(dest.join("src/lib.rs").exists());
        // The generated app's sole entry point is the canonical `Component` +
        // `app!` shape (spec §5.5): no hand-written `android_app!`/`ios_app!`
        // invocation or `App::new` survives in a fresh scaffold.
        let lib_rs = fs::read_to_string(dest.join("src/lib.rs")).unwrap();
        assert!(!lib_rs.contains("android_app!"), "{lib_rs}");
        assert!(!lib_rs.contains("ios_app!"), "{lib_rs}");
        assert!(!lib_rs.contains("App::new"), "{lib_rs}");
        assert!(lib_rs.contains("impl Component for MyAppApp"), "{lib_rs}");
        assert!(lib_rs.contains("forgekit::app!(MyAppApp)"), "{lib_rs}");
        // The generated demo is the Phase 4B notes app: an embedded logo Image, a
        // controlled TextInput whose submit appends a keyed note row (each with a
        // Delete button), inside a scroll view. The `app_logic` keeps the
        // `-> impl View<AppState> + use<>` shape so a fresh scaffold compiles for
        // both mobile targets.
        assert!(
            lib_rs
                .contains("pub fn app_logic(state: &mut AppState) -> impl View<AppState> + use<>")
                && lib_rs.contains("text_input(")
                && lib_rs.contains(".on_submit(")
                && lib_rs.contains("keyed(")
                && lib_rs.contains("Button(\"Delete\"")
                && lib_rs.contains("Image(logo)")
                && lib_rs.contains("scroll_view("),
            "{lib_rs}"
        );
        // The demo bundles a logo asset it decodes once via `include_bytes!`.
        assert!(
            lib_rs.contains("include_bytes!(\"../assets/logo.png\")"),
            "{lib_rs}"
        );
        assert!(dest.join("assets/logo.png").exists());
        assert!(dest.join("src/main.rs").exists());
        assert!(dest.join(".gitignore").exists());
        let gitignore = fs::read_to_string(dest.join(".gitignore")).unwrap();
        // Verify build-output patterns are present (kept in sync with clean.rs REMOVED_DIRS).
        assert!(gitignore.contains("android/app/build/"), "{gitignore}");
        assert!(gitignore.contains("android/.gradle/"), "{gitignore}");
        assert!(gitignore.contains("build/"), "{gitignore}");
        assert!(
            gitignore.contains("android/local.properties"),
            "{gitignore}"
        );
        assert!(dest.join("assets/.gitkeep").exists());
        assert!(!dest.join("template_manifest.json").exists());
        assert!(!dest.join("Cargo.toml.tmpl").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_refuses_non_empty_dest_without_overwrite() {
        let dest = unique_temp_dir("refuse-non-empty");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_context();
        let err = generate(&dest, &ctx, None, false).unwrap_err();
        assert!(err.to_string().contains("existing.txt"), "{err}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_allows_non_empty_dest_with_overwrite() {
        let dest = unique_temp_dir("allow-overwrite");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_context();
        assert!(generate(&dest, &ctx, None, true).is_ok());
        assert!(dest.join("existing.txt").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_allows_empty_existing_dest() {
        let dest = unique_temp_dir("empty-existing");
        fs::create_dir_all(&dest).unwrap();

        let ctx = test_context();
        assert!(generate(&dest, &ctx, None, false).is_ok());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_android_tree_with_stripped_dir_suffix_and_substitutions() {
        let dest = unique_temp_dir("android-tree");
        let ctx = test_context();

        generate(&dest, &ctx, None, false).unwrap();

        // `android.tmpl/` → `android/`: the marker suffix on the source
        // directory doesn't leak into the generated project.
        assert!(dest.join("android").is_dir());
        assert!(!dest.join("android.tmpl").exists());

        let build_gradle = fs::read_to_string(dest.join("android/app/build.gradle.kts")).unwrap();
        assert!(
            build_gradle.contains(&format!("\"{}\"", ctx.android_identifier())),
            "{build_gradle}"
        );

        // `androidIdentifier` path segment expands to nested package
        // directories from the dotted `android_identifier` value.
        let expected_main_activity = dest
            .join("android/app/src/main/kotlin")
            .join(ctx.android_identifier().replace('.', "/"))
            .join("MainActivity.kt");
        assert!(
            expected_main_activity.exists(),
            "expected {}",
            expected_main_activity.display()
        );
        let main_activity = fs::read_to_string(&expected_main_activity).unwrap();
        assert!(
            main_activity.contains(&format!("package {}", ctx.android_identifier())),
            "{main_activity}"
        );

        // `ForgeKitSurfaceView` stays at the fixed `dev/forgekit/` package
        // regardless of `android_identifier`.
        let surface_view =
            dest.join("android/app/src/main/kotlin/dev/forgekit/ForgeKitSurfaceView.kt");
        assert!(surface_view.exists());
        let surface_view_src = fs::read_to_string(&surface_view).unwrap();
        assert!(surface_view_src.contains("package dev.forgekit"));
        assert!(
            surface_view_src.contains(&format!("System.loadLibrary(\"{}\")", ctx.project_name))
        );
        // Touch delivery (spec §9): the `nativeOnTouch` export and its
        // `onTouchEvent` bridge must be present in the generated surface view.
        assert!(
            surface_view_src.contains("external fun nativeOnTouch"),
            "{surface_view_src}"
        );
        assert!(
            surface_view_src.contains("override fun onTouchEvent"),
            "{surface_view_src}"
        );
        // IME transport (spec §14 Phase 4): the three IME exports, the
        // `BaseInputConnection` mirror, and the editor-view hooks must be
        // present, and no AndroidX dependency introduced.
        for needle in [
            "external fun nativeImeApply",
            "external fun nativeImeState",
            "external fun nativeImeAction",
            "override fun onCheckIsTextEditor",
            "override fun onCreateInputConnection",
            "BaseInputConnection(this@ForgeKitSurfaceView",
            "imm.updateSelection",
        ] {
            assert!(
                surface_view_src.contains(needle),
                "expected `{needle}` in generated ForgeKitSurfaceView.kt:\n{surface_view_src}"
            );
        }
        assert!(
            !surface_view_src.contains("androidx"),
            "IME transport must not introduce an AndroidX dependency:\n{surface_view_src}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_ios_tree_with_stripped_dir_suffix_and_substitutions() {
        let dest = unique_temp_dir("ios-tree");
        let ctx = test_context();

        generate(&dest, &ctx, None, false).unwrap();

        // `ios.tmpl/` → `ios/`: the marker suffix on the source directory
        // doesn't leak into the generated project, and `Runner.xcodeproj`
        // (not a `.tmpl` directory) survives intact.
        assert!(dest.join("ios").is_dir());
        assert!(!dest.join("ios.tmpl").exists());

        let pbxproj = dest.join("ios/Runner.xcodeproj/project.pbxproj");
        assert!(pbxproj.exists(), "expected {}", pbxproj.display());
        // The file-level `.tmpl` suffix was stripped and no rendered file
        // kept it.
        assert!(
            !dest
                .join("ios/Runner.xcodeproj/project.pbxproj.tmpl")
                .exists()
        );

        let pbxproj_src = fs::read_to_string(&pbxproj).unwrap();
        // `{{ iosIdentifier }}` rendered to the derived (camelCased) bundle id.
        assert!(
            pbxproj_src.contains(&format!("\"{}\"", ctx.ios_identifier())),
            "{pbxproj_src}"
        );
        // `{{ project_name }}` rendered into the linker flag / staticlib copy.
        assert!(
            pbxproj_src.contains(&format!("-l{}", ctx.project_name)),
            "{pbxproj_src}"
        );
        // No template placeholders survived into the generated project.
        assert!(!pbxproj_src.contains("{{"), "{pbxproj_src}");

        // The shared scheme (headless `-scheme` builds need it) and Info.plist
        // both landed.
        assert!(
            dest.join("ios/Runner.xcodeproj/xcshareddata/xcschemes/Runner.xcscheme")
                .exists()
        );
        let info_plist = fs::read_to_string(dest.join("ios/Runner/Info.plist")).unwrap();
        assert!(
            info_plist.contains("UIApplicationSceneManifest"),
            "{info_plist}"
        );
        assert!(!info_plist.contains("{{"), "{info_plist}");

        // The static Swift sources and bridging header were copied verbatim.
        assert!(dest.join("ios/Runner/AppDelegate.swift").exists());
        assert!(dest.join("ios/Runner/SceneDelegate.swift").exists());
        assert!(dest.join("ios/Runner/ForgeKitView.swift").exists());
        assert!(
            dest.join("ios/Runner/ForgeKitViewController.swift")
                .exists()
        );
        assert!(dest.join("ios/Runner/Runner-Bridging-Header.h").exists());

        // Touch delivery (spec §9): the `forgekit_dispatch_touch` export must be
        // declared in the bridging header and bridged from the view's touch
        // overrides in `ForgeKitView.swift`.
        let bridging =
            fs::read_to_string(dest.join("ios/Runner/Runner-Bridging-Header.h")).unwrap();
        assert!(bridging.contains("forgekit_dispatch_touch"), "{bridging}");
        let forgekit_view = fs::read_to_string(dest.join("ios/Runner/ForgeKitView.swift")).unwrap();
        assert!(forgekit_view.contains("touchesBegan"), "{forgekit_view}");
        assert!(forgekit_view.contains("var onTouch"), "{forgekit_view}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_rejects_org_producing_invalid_ios_bundle_id() {
        let dest = unique_temp_dir("ios-invalid-id");
        let mut ctx = test_context();
        // Whitespace in `org` derives a bundle id with an invalid segment.
        ctx.org = "dev f0x".into();

        let err = generate(&dest, &ctx, None, false).unwrap_err();
        assert!(err.to_string().contains("iOS bundle identifier"), "{err}");
        // Fail-fast: nothing was written.
        assert!(!dest.join("Cargo.toml").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[cfg(unix)]
    #[test]
    fn generate_makes_gradlew_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dest = unique_temp_dir("gradlew-exec");
        let ctx = test_context();

        generate(&dest, &ctx, None, false).unwrap();

        let gradlew = dest.join("android/gradlew");
        let mode = fs::metadata(&gradlew).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o111,
            0o111,
            "gradlew should be executable: {mode:o}"
        );

        // gradlew.bat is a Windows script; it doesn't need a Unix exec bit.
        let gradlew_bat = dest.join("android/gradlew.bat");
        assert!(gradlew_bat.exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_copies_gradle_wrapper_jar_as_a_valid_zip() {
        let dest = unique_temp_dir("wrapper-jar");
        let ctx = test_context();

        generate(&dest, &ctx, None, false).unwrap();

        let jar_bytes = fs::read(dest.join("android/gradle/wrapper/gradle-wrapper.jar")).unwrap();
        // Zip local file header magic — confirms the binary content
        // survived the embed → generate round-trip byte-for-byte.
        assert_eq!(&jar_bytes[0..4], b"PK\x03\x04", "not a valid zip/jar");

        let _ = fs::remove_dir_all(&dest);
    }
}
