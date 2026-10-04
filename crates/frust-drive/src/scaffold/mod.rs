//! Data-driven project scaffolding, modeled on Flutter's `templates/`
//! mechanism: a manifest-listed file tree rendered against a
//! [`TemplateContext`].

pub mod context;
pub mod renderer;

#[allow(unused_imports)]
// NameError/DeepLinkError: public API surface for future callers matching on variants
pub use context::{
    DeepLinkError, DesignSystemContext, FrustDependency, NameError, TemplateContext, title_case,
    validate_deeplink_scheme, validate_project_name,
};

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use include_dir::{Dir, include_dir};

/// The `crates/frust-drive/templates/app/` tree, embedded into the `frust`
/// binary at compile time so `frust create` works standalone without a repo
/// checkout at runtime.
static EMBEDDED_APP_TEMPLATE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates/app");

/// The `crates/frust-drive/templates/design-system/` tree — the second
/// template root [`generate_design_system`] selects, alongside
/// [`EMBEDDED_APP_TEMPLATE`] above (the root [`generate`] renders). A
/// design-system crate is a plain library with no platform project, so it
/// ships as its own tree rather than an `--arch` variant of the app template
/// (which the [`KNOWN_ARCHES`] convention below is scoped to).
static EMBEDDED_DESIGN_SYSTEM_TEMPLATE: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/templates/design-system");

/// Every file path embedded across [`EMBEDDED_APP_TEMPLATE`] and
/// [`EMBEDDED_DESIGN_SYSTEM_TEMPLATE`], each prefixed `app/` or
/// `design-system/` to match what `git ls-files templates` reports relative
/// to this crate's manifest directory. `crates/frust-drive/tests/templates_packaged.rs`
/// diffs this set against the git-tracked set to catch the packaging
/// hazard: a template file `include_dir!` embeds from the working tree that
/// `cargo package` would silently drop because it is untracked or
/// git-ignored (so a published `frust-drive` would embed a file the
/// tarball never shipped).
///
/// Test support only, reached through the `test-util` feature —
/// `crates/frust-drive/tests/templates_packaged.rs` links against the plain
/// library build `cargo test` produces (the same build every downstream
/// binary/test crate links), which carries neither `cfg(test)` (compiled
/// only into the separate unit-test harness build) nor a non-default Cargo
/// feature on its own; this crate's own `Cargo.toml` opts its `tests/`
/// directory in with a self-referencing `frust-drive = { path = ".",
/// features = ["test-util"] }` dev-dependency, the same idiom `frust-cli`'s
/// `Cargo.toml` uses to reach `process::FakeProcessRunner`. Gated
/// `#[cfg(any(test, feature = "test-util"))]` to match that same
/// `FakeProcessRunner` precedent (`crate::process`) — never compiled into a
/// release build, since nothing outside a test binary calls it.
///
/// Note this reflects the directory listing `include_dir!` saw the last
/// time this module was actually recompiled: without the crate's
/// nightly-only `track_path` feature, a file added to `templates/`
/// afterward is invisible to Cargo's freshness check (and thus to this
/// function) until something forces this module to rebuild. That makes
/// this the *fresh-build* guard in `templates_packaged.rs` (the one a
/// clean build, `cargo package --verify`, or CI catches drift with); the
/// test's always-on guard instead walks `templates/` on disk at test time,
/// which needs no recompile to see a newly added file.
#[cfg(any(test, feature = "test-util"))]
pub fn embedded_template_paths() -> Vec<String> {
    let mut paths = Vec::new();
    collect_embedded_paths(&EMBEDDED_APP_TEMPLATE, "app", &mut paths);
    collect_embedded_paths(
        &EMBEDDED_DESIGN_SYSTEM_TEMPLATE,
        "design-system",
        &mut paths,
    );
    paths
}

/// Recursively collects every file path under `dir` into `out`, each
/// prefixed `prefix/`. [`Dir::files`] only yields the current level's
/// files, so this walks [`Dir::dirs`] itself; [`include_dir::File::path`]
/// is already the full path relative to the embedded root (forward-slash
/// normalized regardless of host OS), so no manual join is needed beyond
/// the `app`/`design-system` root prefix.
#[cfg(any(test, feature = "test-util"))]
fn collect_embedded_paths(dir: &Dir<'_>, prefix: &str, out: &mut Vec<String>) {
    for file in dir.files() {
        out.push(format!("{prefix}/{}", file.path().to_string_lossy()));
    }
    for sub in dir.dirs() {
        collect_embedded_paths(sub, prefix, out);
    }
}

/// Name of the manifest file (relative to the template root) that
/// whitelists every file the template ships. Never itself copied into a
/// generated project.
const MANIFEST_FILE: &str = "template_manifest.json";

/// `frust create --arch` tags recognized by [`generate`]. A manifest entry
/// whose mode-stripped logical path ends in `.<tag>` (e.g.
/// `src/lib.rs.clean-signals.tmpl`, logical `src/lib.rs.clean-signals`) is
/// an **arch-scoped variant**: rendered in place of (or, for a filename
/// with no base counterpart, in addition to) the default template's output
/// only when that tag is the selected `arch`, at the un-tagged logical path
/// (`src/lib.rs.clean-signals.tmpl` → `src/lib.rs`). Every other entry is
/// the default template and is skipped only if a same-arch-tagged variant
/// overrides its exact logical path — see [`generate`]'s manifest loop.
/// Only one tag exists today; a future variant adds another entry here plus
/// a matching `frust-cli` `--arch` value arm and template files.
///
/// Public so a front-end (the `frust-tui` create wizard) can enumerate the
/// available architectures without hardcoding them — a new
/// variant appears in the UI with no front-end change.
pub const KNOWN_ARCHES: &[&str] = &["clean-signals"];

/// Logical paths only the default (no `--arch`) template emits: an arch
/// variant lays out its sources differently and must not inherit them.
const DEFAULT_ONLY_PATHS: &[&str] = &["src/home_page.rs"];

/// Splits a mode-stripped logical path into `(base, tag)` if it carries a
/// recognized [`KNOWN_ARCHES`] suffix (see that const's doc for the
/// convention). `None` for a default-template entry.
pub fn split_arch_tag(logical: &str) -> Option<(&str, &str)> {
    KNOWN_ARCHES.iter().find_map(|tag| {
        logical
            .strip_suffix(&format!(".{tag}"))
            .map(|base| (base, *tag))
    })
}

/// A platform project tree the app template can emit — the scaffold's
/// **platform-inclusion axis**, orthogonal to the [`KNOWN_ARCHES`] variant
/// axis above.
///
/// The two axes are genuinely different things, and conflating them is the
/// mistake this type exists to prevent: `--arch` selects *which content*
/// renders at an un-tagged logical path (one app source tree, authored two
/// ways), while a platform selects *whether a whole subtree renders at all*.
/// A platform is never a `.<tag>`-suffixed filename variant; it is the
/// `<platform>.tmpl/` root directory a manifest entry lives under
/// ([`ScaffoldPlatform::template_dir`]), which
/// [`renderer::strip_tmpl_dir_suffixes`] already renders to `<platform>/`.
///
/// Before this axis existed every platform tree was unconditional, which is
/// exactly [`ScaffoldPlatform::DEFAULT`] — see [`generate`], which is
/// defined as [`generate_with_platforms`] against that set precisely so the
/// pre-existing behaviour has a name rather than being reproduced by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScaffoldPlatform {
    Android,
    Ios,
    Macos,
    Windows,
    Linux,
    /// The browser target: `web.tmpl/` → `web/`, the host-page pair
    /// (`index.html` + `frust_web.js`) a `wasm-bindgen` build is assembled
    /// around. The first platform this template ever emitted conditionally.
    Web,
}

impl ScaffoldPlatform {
    /// Every platform the template can emit.
    pub const ALL: &'static [ScaffoldPlatform] = &[
        ScaffoldPlatform::Android,
        ScaffoldPlatform::Ios,
        ScaffoldPlatform::Macos,
        ScaffoldPlatform::Windows,
        ScaffoldPlatform::Linux,
        ScaffoldPlatform::Web,
    ];

    /// The platforms [`generate`] emits: the five that were unconditional
    /// before the axis existed, and **not** [`ScaffoldPlatform::Web`].
    ///
    /// Web is opt-in rather than default because a browser build needs a
    /// wasm toolchain (`wasm32-unknown-unknown` + `wasm-bindgen`) that a
    /// project scaffolded for phones and desktops has no reason to install;
    /// a caller that wants everything asks for [`ScaffoldPlatform::ALL`].
    /// Keeping this set exactly the pre-axis five is also what makes a
    /// default scaffold byte-identical to the pre-axis output.
    pub const DEFAULT: &'static [ScaffoldPlatform] = &[
        ScaffoldPlatform::Android,
        ScaffoldPlatform::Ios,
        ScaffoldPlatform::Macos,
        ScaffoldPlatform::Windows,
        ScaffoldPlatform::Linux,
    ];

    /// The lowercase tag naming this platform on a command line and in the
    /// generated project's own directory layout (`android`, …, `web`).
    pub fn tag(&self) -> &'static str {
        match self {
            ScaffoldPlatform::Android => "android",
            ScaffoldPlatform::Ios => "ios",
            ScaffoldPlatform::Macos => "macos",
            ScaffoldPlatform::Windows => "windows",
            ScaffoldPlatform::Linux => "linux",
            ScaffoldPlatform::Web => "web",
        }
    }

    /// Parses a [`ScaffoldPlatform::tag`] back, for a front-end turning a
    /// user-supplied platform list into this enum. `None` for anything else
    /// — the caller reports the error, since only it knows which flag the
    /// value came from.
    pub fn from_tag(tag: &str) -> Option<ScaffoldPlatform> {
        ScaffoldPlatform::ALL
            .iter()
            .copied()
            .find(|platform| platform.tag() == tag)
    }

    /// The template-root directory this platform's manifest entries live
    /// under (`<tag>.tmpl`).
    pub fn template_dir(&self) -> String {
        format!("{}.tmpl", self.tag())
    }
}

/// Every [`ScaffoldPlatform::tag`], in [`ScaffoldPlatform::ALL`] order.
///
/// Public for the same reason [`KNOWN_ARCHES`] is: a front-end (the
/// `frust-cli` `--platforms` flag, the `frust-tui` create wizard) enumerates
/// the choices from here instead of hardcoding them, so a new platform
/// appears in the UI with no front-end change.
pub fn known_platform_tags() -> Vec<&'static str> {
    ScaffoldPlatform::ALL
        .iter()
        .map(ScaffoldPlatform::tag)
        .collect()
}

/// Splits a manifest entry's logical path into `(platform, remainder)` if
/// its FIRST path component is a `<platform>.tmpl` root directory. `None`
/// for a platform-agnostic entry (`Cargo.toml.tmpl`, `src/lib.rs.tmpl`, the
/// assets) — which is why the check is anchored at the first component and
/// not a substring search.
pub fn split_platform_dir(logical: &str) -> Option<(ScaffoldPlatform, &str)> {
    let (head, rest) = logical.split_once('/')?;
    let platform = ScaffoldPlatform::ALL
        .iter()
        .copied()
        .find(|platform| head == platform.template_dir())?;
    Some((platform, rest))
}

/// How a manifest entry's content should be handled.
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

/// Where to read the template tree from: one of the binary's embedded
/// copies (which of the two [`Dir`]s selects the template *root* — app or
/// design-system), or a filesystem override (`--template-dir`, undocumented,
/// development only).
enum Source<'a> {
    Embedded(&'static Dir<'static>),
    Dir(&'a Path),
}

impl Source<'_> {
    fn manifest(&self) -> Result<Vec<String>> {
        let raw = self.read_to_string(MANIFEST_FILE)?;
        serde_json::from_str(&raw).context("parsing template_manifest.json")
    }

    fn read(&self, relative: &str) -> Result<Vec<u8>> {
        match self {
            Source::Embedded(dir) => dir
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

/// Generates a new app at `dest` from the embedded `crates/frust-drive/templates/app` tree (or
/// `template_dir_override`, for development), emitting
/// [`ScaffoldPlatform::DEFAULT`]'s platform trees. Returns the relative
/// paths written, in manifest order. Refuses a non-empty `dest` unless
/// `overwrite` is set.
///
/// `arch` selects an opt-in template variant (`None` for the default
/// template, provably untouched by this parameter — see
/// [`KNOWN_ARCHES`]'s doc for the manifest convention an arch-scoped entry
/// follows). `Some` value not in [`KNOWN_ARCHES`] is rejected before any
/// file is written.
///
/// The platform-selecting form is [`generate_with_platforms`]; this one is
/// literally that call against [`ScaffoldPlatform::DEFAULT`], so the
/// long-standing "every platform tree is unconditional" behaviour every
/// existing caller depends on is preserved by construction rather than by a
/// parallel code path that could drift from it.
pub fn generate(
    dest: &Path,
    ctx: &TemplateContext,
    template_dir_override: Option<&Path>,
    overwrite: bool,
    arch: Option<&str>,
) -> Result<Vec<PathBuf>> {
    generate_with_platforms(
        dest,
        ctx,
        template_dir_override,
        overwrite,
        arch,
        ScaffoldPlatform::DEFAULT,
    )
}

/// [`generate`], with the platform trees to emit chosen explicitly.
///
/// `platforms` filters the manifest's `<platform>.tmpl/` subtrees (see
/// [`ScaffoldPlatform`]); every platform-agnostic entry — the Cargo
/// manifest, `frust.toml`, `src/`, the assets — is emitted regardless, so an
/// empty `platforms` slice yields a valid app crate with no platform project
/// at all rather than an error. The `arch` axis is applied independently and
/// unchanged: the two selectors never interact, because no platform subtree
/// carries an arch-tagged entry.
///
/// Order is unaffected: entries are still written in manifest order, with
/// the excluded ones simply skipped.
pub fn generate_with_platforms(
    dest: &Path,
    ctx: &TemplateContext,
    template_dir_override: Option<&Path>,
    overwrite: bool,
    arch: Option<&str>,
    platforms: &[ScaffoldPlatform],
) -> Result<Vec<PathBuf>> {
    check_destination(dest, overwrite)?;

    if let Some(tag) = arch
        && !KNOWN_ARCHES.contains(&tag)
    {
        bail!(
            "unknown --arch `{tag}` (expected one of: {})",
            KNOWN_ARCHES.join(", ")
        );
    }

    // Fail fast: a grammatically invalid derived iOS bundle identifier is
    // rejected before any file is written, rather than
    // emitting an Xcode project that `xcodebuild` would later refuse.
    ctx.validate_ios_identifier().map_err(|err| {
        anyhow!("invalid iOS bundle identifier derived from `org` + project name: {err}")
    })?;

    let source = match template_dir_override {
        Some(dir) => Source::Dir(dir),
        None => Source::Embedded(&EMBEDDED_APP_TEMPLATE),
    };
    let manifest = source.manifest()?;
    // Platform-inclusion axis, threaded into the render context too: a
    // platform-agnostic file that is ALWAYS emitted (`Cargo.toml`) can still
    // carry content — the `windows/build.rs` wiring — that only makes sense
    // when a particular platform subtree exists, so every `.tmpl` file gets
    // one `{{ platform_<tag> }}` boolean-shaped var per
    // `ScaffoldPlatform::ALL` entry, not just the coarser whole-subtree
    // skip below. See `context::platform_render_vars`'s doc.
    let mut render_vars = ctx.render_vars();
    render_vars.extend(context::platform_render_vars(platforms));
    let path_vars = ctx.path_vars();

    let mut written = Vec::with_capacity(manifest.len());
    for entry in &manifest {
        let (mode, logical) = classify(entry);

        // Platform-inclusion axis: an entry under a `<platform>.tmpl/` root
        // is emitted only when that platform is selected. Checked before
        // arch resolution because the two axes are independent — no platform
        // subtree carries an arch-tagged entry — and a de-selected platform's
        // entries need no further consideration at all.
        if let Some((platform, _)) = split_platform_dir(logical)
            && !platforms.contains(&platform)
        {
            continue;
        }

        // Default-only files (`DEFAULT_ONLY_PATHS`) are not emitted for any
        // `--arch` variant: that variant supplies its own source layout.
        if arch.is_some() && DEFAULT_ONLY_PATHS.contains(&logical) {
            continue;
        }

        // Arch-tag resolution: an entry whose logical path carries a
        // recognized `.{arch}` suffix only renders when that tag is the
        // selected `arch` (skipped entirely otherwise), landing at the
        // un-tagged logical path; an untagged (default) entry is skipped
        // only when a same-arch-tagged variant overrides its exact logical
        // path — see `KNOWN_ARCHES`'s doc. `arch == None` never skips an
        // untagged entry and never matches a tagged one, so the default
        // render path (this whole branch) is unaffected by arch selection.
        let target_logical: String = if let Some((base, tag)) = split_arch_tag(logical) {
            if Some(tag) != arch {
                continue;
            }
            base.to_string()
        } else if let Some(tag) = arch {
            let overridden = manifest
                .iter()
                .any(|other| split_arch_tag(classify(other).1) == Some((logical, tag)));
            if overridden {
                continue;
            }
            logical.to_string()
        } else {
            logical.to_string()
        };

        let out_relative = write_entry(
            &source,
            entry,
            &mode,
            &target_logical,
            dest,
            &render_vars,
            &path_vars,
        )?;
        written.push(out_relative);
    }
    Ok(written)
}

/// Generates a new out-of-tree **design-system** crate at `dest` from the
/// embedded `crates/frust-drive/templates/design-system` tree (or `template_dir_override`, for
/// development). Returns the relative paths written, in manifest order.
/// Refuses a non-empty `dest` unless `overwrite` is set — the same contract
/// [`generate`] carries for an app scaffold.
///
/// This is the counterpart [`generate`]'s own doc comment references: the
/// design-system template root selected via [`DesignSystemContext`] instead
/// of [`TemplateContext`]. A design-system crate is a plain library with no
/// platform project and no `--arch` variant of its own, so this entry point
/// carries neither an `arch` parameter nor the iOS-identifier fail-fast
/// [`generate`] runs before writing.
pub fn generate_design_system(
    dest: &Path,
    ctx: &DesignSystemContext,
    template_dir_override: Option<&Path>,
    overwrite: bool,
) -> Result<Vec<PathBuf>> {
    check_destination(dest, overwrite)?;

    let source = match template_dir_override {
        Some(dir) => Source::Dir(dir),
        None => Source::Embedded(&EMBEDDED_DESIGN_SYSTEM_TEMPLATE),
    };
    let manifest = source.manifest()?;
    let render_vars = ctx.render_vars();
    let path_vars = ctx.path_vars();

    let mut written = Vec::with_capacity(manifest.len());
    for entry in &manifest {
        let (mode, logical) = classify(entry);
        let out_relative = write_entry(
            &source,
            entry,
            &mode,
            logical,
            dest,
            &render_vars,
            &path_vars,
        )?;
        written.push(out_relative);
    }
    Ok(written)
}

/// Renders/copies one manifest `entry` to `dest`, given its (already
/// arch-resolved, for the app root) logical output path. Shared by
/// [`generate`] and [`generate_design_system`] — the design-system entry
/// point has no arch selector, so it calls this directly with its own
/// manifest entry's logical path.
fn write_entry(
    source: &Source,
    entry: &str,
    mode: &FileMode,
    target_logical: &str,
    dest: &Path,
    render_vars: &BTreeMap<&str, String>,
    path_vars: &BTreeMap<&str, String>,
) -> Result<PathBuf> {
    // A source-tree directory (e.g. `android.tmpl/`) may itself carry a
    // `.tmpl` suffix as a purely organizational marker; strip it before
    // path-placeholder expansion so it doesn't leak into the generated
    // project (`android.tmpl/` → `android/`).
    let target_logical = renderer::strip_tmpl_dir_suffixes(Path::new(target_logical));
    let out_relative = renderer::expand_path(&target_logical, path_vars);
    let out_path = dest.join(&out_relative);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating directory `{}`", parent.display()))?;
    }
    match mode {
        FileMode::Render => {
            let raw = source.read_to_string(entry)?;
            let rendered = renderer::render(&raw, render_vars)
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
    Ok(out_relative)
}

/// Filenames the scaffold vendors verbatim that must retain their
/// executable bit in the generated project. `fs::write` always creates
/// files with the umask-default (non-executable) mode — `include_dir` has
/// no permission metadata to read back from the embedded copy, so
/// "preserve the original mode" isn't available and this has to be a
/// filename allowlist instead. Currently only the Gradle wrapper script
/// (`android.tmpl/gradlew`); its Windows counterpart (`gradlew.bat`)
/// doesn't need a Unix exec bit.
#[cfg(unix)]
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
    use crate::build_dirs::BuildLayout;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("frust-cli-test-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn test_context() -> TemplateContext {
        TemplateContext {
            project_name: "my_app".into(),
            title_case_name: "My App".into(),
            org: "dev.f0x".into(),
            description: "A new Frust application.".into(),
            frust_version: "0.1.0".into(),
            frust: FrustDependency::Path("/path/to/frust".into()),
            deeplink_scheme: None,
            deeplink_host: None,
        }
    }

    fn registry_test_context() -> TemplateContext {
        TemplateContext {
            frust: FrustDependency::Registry {
                version: "0.5.0".into(),
            },
            ..test_context()
        }
    }

    fn test_design_system_context() -> DesignSystemContext {
        DesignSystemContext {
            name: "acme_design".into(),
            frust_version: "0.1.0".into(),
            frust: FrustDependency::Path("/path/to/frust".into()),
        }
    }

    /// Registry mode (the `frust create` default): the facade is the
    /// published `frust-ui` package imported as `frust`, and every plugin is a
    /// plain version requirement. See https://github.com/lloydmeta/frunk/issues/258.
    #[test]
    fn registry_mode_renders_version_dependencies() {
        let dest = unique_temp_dir("registry-mode");
        let ctx = registry_test_context();
        generate(&dest, &ctx, None, false, None).unwrap();

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        for line in [
            "frust = { package = \"frust-ui\", version = \"0.5.0\" }",
            "frust-material = \"0.5.0\"",
        ] {
            assert!(
                cargo_toml.lines().any(|l| l == line),
                "missing `{line}` in {cargo_toml}"
            );
        }
        assert!(!cargo_toml.contains("path ="), "{cargo_toml}");
        let manifest: toml::Value = toml::from_str(&cargo_toml).expect("manifest parses");
        assert_eq!(
            manifest["dependencies"]["frust"]["package"].as_str(),
            Some("frust-ui")
        );

        let clean = unique_temp_dir("registry-mode-clean-signals");
        generate(&clean, &ctx, None, false, Some("clean-signals")).unwrap();
        let cargo_toml = fs::read_to_string(clean.join("Cargo.toml")).unwrap();
        for line in [
            "frust = { package = \"frust-ui\", version = \"0.5.0\" }",
            "frust-material = \"0.5.0\"",
            "clean-signals-frust = \"0.5.0\"",
        ] {
            assert!(
                cargo_toml.lines().any(|l| l == line),
                "missing `{line}` in {cargo_toml}"
            );
        }
        assert!(!cargo_toml.contains("path ="), "{cargo_toml}");

        let ds = unique_temp_dir("registry-mode-design-system");
        let ds_ctx = DesignSystemContext {
            frust: ctx.frust.clone(),
            ..test_design_system_context()
        };
        generate_design_system(&ds, &ds_ctx, None, false).unwrap();
        let cargo_toml = fs::read_to_string(ds.join("Cargo.toml")).unwrap();
        assert!(
            cargo_toml
                .lines()
                .any(|l| l == "frust = { package = \"frust-ui\", version = \"0.5.0\" }"),
            "{cargo_toml}"
        );
    }

    #[test]
    fn generate_produces_manifest_listed_files_with_substitutions() {
        let dest = unique_temp_dir("manifest-set");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false, None).unwrap();
        assert!(!written.is_empty());

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("my_app"), "{cargo_toml}");
        // The app crate builds the Android `cdylib` (JNI bridge), the iOS
        // `staticlib` (C-ABI bridge), and an `rlib` for the desktop preview
        // binary.
        assert!(
            cargo_toml.contains("crate-type = [\"cdylib\", \"staticlib\", \"rlib\"]"),
            "{cargo_toml}"
        );
        // Material 3 ships as a sibling plugin crate dependency, not a `frust`
        // Cargo feature: the `frust` edge itself names no features at all
        // (`frust` carries no design-system catalog, `default = []`).
        assert!(
            cargo_toml.contains("frust = { package = \"frust-ui\", path = \"/path/to/frust\" }"),
            "{cargo_toml}"
        );
        assert!(
            cargo_toml
                .contains("frust-material = { path = \"/path/to/frust/../../plugins/material\" }"),
            "{cargo_toml}"
        );
        assert!(!cargo_toml.contains("frust-glyph"), "{cargo_toml}");
        assert!(
            !cargo_toml.contains("frust-shared-preferences"),
            "{cargo_toml}"
        );
        assert!(!cargo_toml.contains("[\"glyph\""), "{cargo_toml}");
        assert!(!cargo_toml.contains("\"material\""), "{cargo_toml}");
        assert!(!cargo_toml.contains("\"cupertino\""), "{cargo_toml}");
        assert!(!cargo_toml.contains("glyph-fonts"), "{cargo_toml}");
        let frust_toml = fs::read_to_string(dest.join("frust.toml")).unwrap();
        assert!(frust_toml.contains("dev.f0x"), "{frust_toml}");

        assert!(dest.join("src/lib.rs").exists());
        // The generated app's sole entry point is the canonical `Component` +
        // `app!` shape: no hand-written `android_app!`/`ios_app!` invocation
        // or `App::new` survives in a fresh scaffold.
        let lib_rs = fs::read_to_string(dest.join("src/lib.rs")).unwrap();
        assert!(!lib_rs.contains("android_app!"), "{lib_rs}");
        assert!(!lib_rs.contains("ios_app!"), "{lib_rs}");
        assert!(!lib_rs.contains("App::new"), "{lib_rs}");
        assert!(lib_rs.contains("impl Component for MyAppApp"), "{lib_rs}");
        // The generated app ships Material 3 by default: the `app!` invocation's
        // `setup` block is what actually seeds it as the active theme (the
        // `frust-material` dependency alone only compiles the code in).
        assert!(
            lib_rs.contains(
                "frust::app!(\n    MyAppApp,\n    setup = {\n        frust_material::install();\n    }\n);"
            ),
            "{lib_rs}"
        );
        // The generated demo is the Flutter-style counter: the root component
        // hosts a `HomePage` component (its own module) built from widget fns.
        assert!(
            lib_rs.contains("component(") && lib_rs.contains("mod home_page"),
            "{lib_rs}"
        );
        let home_rs = fs::read_to_string(dest.join("src/home_page.rs")).unwrap();
        assert!(
            home_rs.contains("impl Component for HomePage")
                && home_rs.contains("scaffold(")
                && home_rs.contains("app_bar")
                && home_rs.contains("fab(")
                && home_rs.contains("icons::ADD"),
            "{home_rs}"
        );
        for src in [&lib_rs, &home_rs] {
            assert!(!src.contains("app_logic"), "{src}");
            assert!(!src.contains("frust-glyph"), "{src}");
        }
        // lib.rs's comment may name the swap; the live setup call must not.
        assert!(
            !lib_rs.contains("        frust_glyph::install();"),
            "{lib_rs}"
        );
        assert!(!home_rs.contains("frust_glyph"), "{home_rs}");
        // The launcher icon still ships even though the UI no longer shows it.
        assert!(dest.join("assets/logo.png").exists());
        assert!(dest.join("src/main.rs").exists());
        assert!(dest.join(".gitignore").exists());
        let gitignore = fs::read_to_string(dest.join(".gitignore")).unwrap();
        // Verify build-output patterns are present (kept in sync with
        // `build_dirs::{CLEAN_DIRS, LEGACY_CLEAN_DIRS}`).
        assert!(gitignore.contains("android/app/build/"), "{gitignore}");
        // The `:frust-embedding` module's redirected Gradle output.
        assert!(gitignore.contains("android/build/"), "{gitignore}");
        assert!(gitignore.contains("android/.gradle/"), "{gitignore}");
        assert!(gitignore.contains("build/"), "{gitignore}");
        assert!(
            gitignore.contains("android/local.properties"),
            "{gitignore}"
        );
        // Xcode's per-user state, written whenever the project is opened.
        assert!(gitignore.contains("xcuserdata/"), "{gitignore}");
        // The machine-local Swift package symlinks `platform_wiring` maintains
        // (`ios/FrustEmbedding`, and one per plugin package).
        assert!(
            gitignore.lines().any(|line| line == "/ios/Frust*"),
            "{gitignore}"
        );
        assert!(dest.join("assets/.gitkeep").exists());
        // The Android link-flags file: a dot-directory manifest entry
        // (`.cargo/config.toml`), carried verbatim so a scaffolded app
        // inherits `--pack-dyn-relocs=android`/`--icf=all` unchanged from the
        // embedded template this crate owns. Structural agreement between
        // the template and this checkout's own root `.cargo/config.toml` is
        // a separate concern this crate has no reach into — it is
        // `crates/frust-cli/tests/profile_sync.rs`'s
        // `android_cargo_config_identical_between_root_and_template`.
        assert!(dest.join(".cargo/config.toml").exists());
        let cargo_config = fs::read_to_string(dest.join(".cargo/config.toml")).unwrap();
        let embedded_cargo_config = EMBEDDED_APP_TEMPLATE
            .get_file(".cargo/config.toml")
            .expect("embedded template must carry .cargo/config.toml")
            .contents_utf8()
            .expect(".cargo/config.toml must be valid UTF-8");
        assert_eq!(
            cargo_config, embedded_cargo_config,
            "the scaffolded .cargo/config.toml must be byte-identical to the \
             embedded `crates/frust-drive/templates/app/.cargo/config.toml`"
        );
        assert!(!dest.join("template_manifest.json").exists());
        assert!(!dest.join("Cargo.toml.tmpl").exists());

        // README.md generated with theme/font docs and examples
        let readme = fs::read_to_string(dest.join("README.md")).unwrap();
        assert!(readme.contains("Material 3 design system"), "{readme}");
        assert!(readme.contains("home_page.rs"), "{readme}");
        assert!(readme.contains("frust_material::install()"), "{readme}");
        assert!(readme.contains("frust_cupertino::install()"), "{readme}");
        assert!(readme.contains("register_app_fonts"), "{readme}");
        assert!(readme.contains("Theme::builder"), "{readme}");
        assert!(readme.contains("frust::register_app_fonts("), "{readme}");
        assert!(!readme.contains("bundles no font"), "{readme}");
        // Verify no unresolved {{ }} placeholders remain
        assert!(!readme.contains("{{"), "{readme}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_refuses_non_empty_dest_without_overwrite() {
        let dest = unique_temp_dir("refuse-non-empty");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_context();
        let err = generate(&dest, &ctx, None, false, None).unwrap_err();
        assert!(err.to_string().contains("existing.txt"), "{err}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_allows_non_empty_dest_with_overwrite() {
        let dest = unique_temp_dir("allow-overwrite");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_context();
        assert!(generate(&dest, &ctx, None, true, None).is_ok());
        assert!(dest.join("existing.txt").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_allows_empty_existing_dest() {
        let dest = unique_temp_dir("empty-existing");
        fs::create_dir_all(&dest).unwrap();

        let ctx = test_context();
        assert!(generate(&dest, &ctx, None, false, None).is_ok());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_android_tree_with_stripped_dir_suffix_and_substitutions() {
        let dest = unique_temp_dir("android-tree");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

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
        // The generated `MainActivity` is a near-empty subclass of the
        // framework-owned `dev.frust.FrustActivity` — every
        // lifecycle/deep-link/back/IME behavior it used to carry inline now
        // lives in `crates/frust-shell-android/platform/android/frust-embedding` (covered by that
        // module's own compile gate and the on-device run, not by this test).
        assert!(
            main_activity.contains("import dev.frust.FrustActivity"),
            "{main_activity}"
        );
        assert!(
            main_activity.contains("class MainActivity : FrustActivity()"),
            "{main_activity}"
        );
        assert!(
            main_activity.lines().count() <= 6,
            "MainActivity.kt must stay under the 6-line budget:\n{main_activity}"
        );

        // No framework Kotlin/Java is rendered into the app any more.
        assert!(
            !dest.join("android/app/src/main/kotlin/dev/frust").exists(),
            "the app template must ship no `dev.frust` Kotlin"
        );
        assert!(
            !dest
                .join("android/app/src/main/java/dev/accesskit")
                .exists(),
            "the app template must ship no vendored accesskit delegate"
        );

        // The embedding module is wired in by path: an `include`, a
        // `projectDir` resolved through the machine-local-directory helper,
        // and the build-output redirect that keeps the shared checkout
        // pristine. The helper is the very text `frust plugin add` inserts
        // into a settings file that predates it.
        let settings = fs::read_to_string(dest.join("android/settings.gradle.kts")).unwrap();
        for needle in [
            crate::plugin::apply::SETTINGS_LOCAL_DIR_HELPER,
            "include(\":frust-embedding\")",
            "project(\":frust-embedding\").projectDir = frustLocalDir(\"frust.embedding.dir\")",
            "gradle.lifecycle.beforeProject {",
            "layout.buildDirectory.set(rootDir.resolve(\"../build/android/app\"))",
            "layout.buildDirectory.set(rootDir.resolve(\"../build/android/frust-embedding\"))",
        ] {
            assert!(
                settings.contains(needle),
                "expected `{needle}`:\n{settings}"
            );
        }
        // The helper is defined before its first use, and the plugin anchor
        // stays below every built-in module.
        let helper_at = settings.find("fun frustLocalDir(").unwrap();
        let use_at = settings
            .find("frustLocalDir(\"frust.embedding.dir\")")
            .unwrap();
        let anchor_at = settings.find("// frust:plugin-includes").unwrap();
        assert!(helper_at < use_at && use_at < anchor_at, "{settings}");
        assert!(!settings.contains("gradleProperty"), "{settings}");

        // No machine-local location in a tracked file: `gradle.properties`
        // carries no `frust.embedding.dir` (the key lives in the gitignored
        // `local.properties`, written by `platform_wiring::sync`), and
        // neither file names an absolute path.
        let gradle_properties = fs::read_to_string(dest.join("android/gradle.properties")).unwrap();
        assert!(
            !gradle_properties.contains(crate::platform_wiring::EMBEDDING_DIR_KEY),
            "{gradle_properties}"
        );
        assert!(!gradle_properties.contains("{{"), "{gradle_properties}");
        assert!(!gradle_properties.contains('\\'), "{gradle_properties}");
        assert!(
            !dest.join(crate::platform_wiring::LOCAL_PROPERTIES).exists(),
            "the scaffold itself writes no machine-local file"
        );

        assert!(
            build_gradle.contains("implementation(project(\":frust-embedding\"))"),
            "{build_gradle}"
        );

        // `FrustActivity` resolves the app's native library name from this
        // `<meta-data>`; its absence is a launch-time `check()` failure.
        let manifest =
            fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            manifest.contains(&format!(
                "<meta-data android:name=\"dev.frust.nativeLibrary\" android:value=\"{}\" />",
                ctx.project_name
            )),
            "{manifest}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_debug_and_profile_manifest_overlays_granting_internet_only() {
        let dest = unique_temp_dir("android-debug-profile-manifests");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false, None).unwrap();

        for overlay in [
            "android/app/src/debug/AndroidManifest.xml",
            "android/app/src/profile/AndroidManifest.xml",
        ] {
            assert!(
                written.iter().any(|p| p == Path::new(overlay)),
                "expected `{overlay}` in written paths: {written:?}"
            );
            let contents = fs::read_to_string(dest.join(overlay)).unwrap();
            assert!(
                contents
                    .contains("<uses-permission android:name=\"android.permission.INTERNET\" />"),
                "{contents}"
            );
            assert!(!contents.contains("{{"), "{contents}");
        }

        // Release keeps the empty permission set: no `src/release/` overlay
        // ships, and the main manifest itself declares none.
        assert!(!dest.join("android/app/src/release").exists());
        let main_manifest =
            fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            !main_manifest.contains("uses-permission"),
            "{main_manifest}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_ios_tree_with_stripped_dir_suffix_and_substitutions() {
        let dest = unique_temp_dir("ios-tree");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

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

        // `ios/Runner/` ships exactly four things — the two thin delegates,
        // the Info.plist and the asset catalog. Every framework Swift
        // source (and the bridging header) now lives in the
        // `crates/frust-shell-ios/platform/ios/FrustEmbedding` Swift package.
        let mut runner_entries: Vec<String> = fs::read_dir(dest.join("ios/Runner"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        runner_entries.sort();
        assert_eq!(
            runner_entries,
            [
                "AppDelegate.swift",
                "Assets.xcassets",
                "Info.plist",
                "SceneDelegate.swift",
            ],
            "the app template must ship no framework Swift under ios/Runner"
        );

        // The two delegates are near-empty subclasses of the package's own
        // base classes. Their former bodies — the deep-link plumbing in
        // particular (`connectionOptions.urlContexts`, `openURLContexts`,
        // `handleDeepLink`) — are now `FrustSceneDelegate`/
        // `FrustViewController` code, covered by the package's compile gate
        // and the on-device deep-link run, not by this test.
        let app_delegate = fs::read_to_string(dest.join("ios/Runner/AppDelegate.swift")).unwrap();
        assert!(
            app_delegate.contains("import FrustEmbedding"),
            "{app_delegate}"
        );
        assert!(
            app_delegate.contains("class AppDelegate: FrustAppDelegate"),
            "{app_delegate}"
        );
        assert!(
            app_delegate.lines().count() <= 6,
            "AppDelegate.swift must stay under the 6-line budget:\n{app_delegate}"
        );

        let scene_delegate =
            fs::read_to_string(dest.join("ios/Runner/SceneDelegate.swift")).unwrap();
        assert!(
            scene_delegate.contains("import FrustEmbedding"),
            "{scene_delegate}"
        );
        assert!(
            scene_delegate.contains("class SceneDelegate: FrustSceneDelegate"),
            "{scene_delegate}"
        );
        assert!(
            scene_delegate.lines().count() <= 4,
            "SceneDelegate.swift must stay under the 4-line budget:\n{scene_delegate}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// A 24-character uppercase-hex pbxproj object id — the style every id
    /// in the iOS template uses.
    fn is_pbx_object_id(token: &str) -> bool {
        token.len() == 24
            && token
                .chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
    }

    /// Every object-id token on one pbxproj line, in order of appearance.
    fn pbx_ids_on_line(line: &str) -> Vec<&str> {
        line.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|token| is_pbx_object_id(token))
            .collect()
    }

    /// Splits a rendered `project.pbxproj` into `(defined ids, referenced
    /// ids)`. A *definition* is a top-level entry of the `objects = { … }`
    /// map: exactly two leading tabs, an object id, then ` = {`. Every other
    /// id mention is a *reference* — including the deeper-indented
    /// `TargetAttributes` entry and the trailing `rootObject` line, both of
    /// which point at objects defined elsewhere.
    fn pbx_ids(pbxproj: &str) -> (Vec<String>, Vec<String>) {
        let mut defined = Vec::new();
        let mut referenced = Vec::new();
        for line in pbxproj.lines() {
            let ids = pbx_ids_on_line(line);
            let top_level = line
                .strip_prefix("\t\t")
                .filter(|rest| !rest.starts_with('\t'));
            let is_definition = top_level.is_some_and(|rest| {
                ids.first().is_some_and(|id| rest.starts_with(id)) && rest.contains(" = {")
            });
            let mut ids = ids.into_iter();
            if is_definition {
                defined.push(ids.next().expect("a definition line has an id").to_string());
            }
            referenced.extend(ids.map(str::to_string));
        }
        (defined, referenced)
    }

    /// The body of one `/* Begin <name> section */ … /* End <name> section */`
    /// block, so a section-scoped assertion can't accidentally match text
    /// from a different section.
    fn pbx_section<'a>(pbxproj: &'a str, name: &str) -> &'a str {
        let begin = format!("/* Begin {name} section */");
        let end = format!("/* End {name} section */");
        let start = pbxproj
            .find(&begin)
            .unwrap_or_else(|| panic!("missing `{begin}`:\n{pbxproj}"));
        let stop = pbxproj
            .find(&end)
            .unwrap_or_else(|| panic!("missing `{end}`:\n{pbxproj}"));
        &pbxproj[start..stop]
    }

    /// The id introduced by the single definition line containing `needle`.
    fn pbx_definition_id(pbxproj: &str, needle: &str) -> String {
        let matches: Vec<&str> = pbxproj
            .lines()
            .filter(|line| line.contains(needle))
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one definition line containing `{needle}`:\n{pbxproj}"
        );
        pbx_ids_on_line(matches[0])
            .first()
            .unwrap_or_else(|| panic!("no object id on `{}`", matches[0]))
            .to_string()
    }

    /// Retargeted from the platform-view host-file registration this test
    /// originally guarded: the generated iOS project must wire the
    /// `FrustEmbedding` Swift package consistently
    /// across **all five** sections that have to mention it
    /// (`PBXProject.packageReferences`, the `XCLocalSwiftPackageReference`,
    /// `PBXNativeTarget.packageProductDependencies`, the
    /// `XCSwiftPackageProductDependency`, and a `PBXBuildFile { productRef }`
    /// in the Frameworks phase), must mention none of the six deleted
    /// framework files, and must carry no dangling object id — the classic
    /// corruption that makes Xcode refuse to open a project.
    ///
    /// Host-runnable: this checks the *rendered* text without needing
    /// `xcodebuild` (which `create_ios.rs` gates on macOS), and on a non-macOS
    /// host it is the **only** automated guard the pbxproj has.
    #[test]
    fn generate_ios_pbxproj_wires_embedding_package_well_formed() {
        let dest = unique_temp_dir("ios-pbxproj-embedding");
        let ctx = test_context();
        generate(&dest, &ctx, None, false, None).unwrap();

        let pbxproj =
            fs::read_to_string(dest.join("ios/Runner.xcodeproj/project.pbxproj")).unwrap();

        // (1) The local package reference itself, naming the
        // `ios/FrustEmbedding` symlink `platform_wiring::sync` points at the
        // resolved shell crate's package — a path relative to `ios/`, the
        // directory containing `Runner.xcodeproj`.
        let package_ref_id = pbx_definition_id(
            &pbxproj,
            "/* XCLocalSwiftPackageReference \"FrustEmbedding\" */ = {",
        );
        let package_ref_section = pbx_section(&pbxproj, "XCLocalSwiftPackageReference");
        assert!(
            package_ref_section.contains("isa = XCLocalSwiftPackageReference;"),
            "{pbxproj}"
        );
        assert!(
            package_ref_section.contains("relativePath = \"FrustEmbedding\";"),
            "{pbxproj}"
        );

        // (2) …referenced from `PBXProject.packageReferences`.
        assert!(
            pbx_section(&pbxproj, "PBXProject").contains(&package_ref_id),
            "`packageReferences` must list the local package reference:\n{pbxproj}"
        );

        // (3) The product dependency on the package's `FrustEmbedding`
        // product…
        let product_dep_id = pbx_definition_id(&pbxproj, "/* FrustEmbedding */ = {");
        assert!(
            pbx_section(&pbxproj, "XCSwiftPackageProductDependency")
                .contains("productName = FrustEmbedding;"),
            "{pbxproj}"
        );

        // (4) …referenced from `PBXNativeTarget.packageProductDependencies`…
        assert!(
            pbx_section(&pbxproj, "PBXNativeTarget").contains(&product_dep_id),
            "`packageProductDependencies` must list the product dependency:\n{pbxproj}"
        );

        // (5) …and linked by a `PBXBuildFile { productRef = … }` in the
        // Frameworks build phase (all five pieces are required for Xcode to
        // link the package in).
        let build_file_id = pbx_definition_id(
            &pbxproj,
            "/* FrustEmbedding in Frameworks */ = {isa = PBXBuildFile;",
        );
        assert!(
            pbxproj.contains(&format!("productRef = {product_dep_id} ")),
            "the Frameworks PBXBuildFile must point at the product dependency:\n{pbxproj}"
        );
        assert!(
            pbx_section(&pbxproj, "PBXFrameworksBuildPhase").contains(&build_file_id),
            "the Frameworks phase must list the product build file:\n{pbxproj}"
        );

        // None of the six deleted framework files may survive in *any*
        // section, and the bridging-header build setting is gone outright.
        for deleted in [
            "FrustView.swift",
            "FrustViewController.swift",
            "FrustViewHost.swift",
            "FrustPlatformViewFactory.swift",
            "FrustTextInput.swift",
            "Runner-Bridging-Header.h",
        ] {
            assert!(
                !pbxproj.contains(deleted),
                "`{deleted}` is deleted but still referenced in the pbxproj:\n{pbxproj}"
            );
            assert!(
                !dest.join("ios/Runner").join(deleted).exists(),
                "`{deleted}` must not be generated any more"
            );
        }
        assert!(!pbxproj.contains("SWIFT_OBJC_BRIDGING_HEADER"), "{pbxproj}");

        // Over-deletion guard: the native build path is untouched.
        assert!(pbxproj.contains("Build Rust staticlib"), "{pbxproj}");
        assert_eq!(
            pbxproj
                .matches("LIBRARY_SEARCH_PATHS = \"$(inherited) $(BUILT_PRODUCTS_DIR)\";")
                .count(),
            3,
            "{pbxproj}"
        );
        assert_eq!(
            pbxproj
                .matches(&format!("\"-l{}\",", ctx.project_name))
                .count(),
            3,
            "{pbxproj}"
        );
        assert_eq!(
            pbxproj
                .matches("IPHONEOS_DEPLOYMENT_TARGET = 15.0;")
                .count(),
            3,
            "{pbxproj}"
        );
        assert!(pbxproj.contains("objectVersion = 54;"), "{pbxproj}");

        // Balanced: every `/* Begin X section */` has a matching `/* End */`.
        assert_eq!(
            pbxproj.matches("/* Begin ").count(),
            pbxproj.matches("/* End ").count(),
            "unbalanced PBX sections:\n{pbxproj}"
        );

        // Object-id consistency, the corruption this test exists to catch:
        // no duplicate definition, no reference to an undefined id (a
        // dangling id — the project Xcode refuses to open), and no defined
        // object nothing points at (an orphan left by a partial deletion).
        let (defined, referenced) = pbx_ids(&pbxproj);
        assert!(defined.len() > 20, "suspiciously few objects:\n{pbxproj}");
        let mut unique = defined.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            defined.len(),
            "duplicate PBX object id(s) in generated pbxproj:\n{pbxproj}"
        );
        for id in &referenced {
            assert!(
                defined.contains(id),
                "dangling PBX object id `{id}` (referenced but never defined):\n{pbxproj}"
            );
        }
        for id in &defined {
            assert!(
                referenced.contains(id),
                "orphaned PBX object `{id}` (defined but never referenced):\n{pbxproj}"
            );
        }

        // No template placeholders survived rendering.
        assert!(!pbxproj.contains("{{"), "{pbxproj}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// With no `[deeplink]` config, the rendered Android manifest must be
    /// byte-identical to its pre-deep-link-support rendering (no
    /// `android:launchMode`, no second `<intent-filter>`). The baseline
    /// carries the `dev.frust.nativeLibrary` `<meta-data>` element —
    /// `FrustActivity` reads it to resolve the app's Rust library name.
    #[test]
    fn generate_android_manifest_without_deeplink_is_byte_identical_to_pre_task_baseline() {
        let dest = unique_temp_dir("manifest-no-deeplink");
        let ctx = test_context();
        assert!(ctx.deeplink_scheme.is_none());

        generate(&dest, &ctx, None, false, None).unwrap();

        let manifest =
            fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        let expected = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <manifest xmlns:android=\"http://schemas.android.com/apk/res/android\">\n\
             \n\
             \x20\x20\x20\x20<application\n\
             \x20\x20\x20\x20\x20\x20\x20\x20android:allowBackup=\"true\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20android:icon=\"@mipmap/ic_launcher\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20android:label=\"{title}\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20android:roundIcon=\"@mipmap/ic_launcher_round\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20android:theme=\"@android:style/Theme.Material.NoActionBar\">\n\
             \n\
             \x20\x20\x20\x20\x20\x20\x20\x20<!-- Read by dev.frust.FrustActivity to load this app's Rust library. -->\n\
             \x20\x20\x20\x20\x20\x20\x20\x20<meta-data android:name=\"dev.frust.nativeLibrary\" android:value=\"{project}\" />\n\
             \n\
             \x20\x20\x20\x20\x20\x20\x20\x20<activity\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20android:name=\".MainActivity\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20android:exported=\"true\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20android:configChanges=\"orientation|screenSize|keyboardHidden|uiMode\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20android:windowSoftInputMode=\"adjustResize\"\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20android:theme=\"@android:style/Theme.Material.NoActionBar\">\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20<intent-filter>\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20<action android:name=\"android.intent.action.MAIN\" />\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20<category android:name=\"android.intent.category.LAUNCHER\" />\n\
             \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20</intent-filter>\n\
             \x20\x20\x20\x20\x20\x20\x20\x20</activity>\n\
             \x20\x20\x20\x20</application>\n\
             \n\
             </manifest>\n",
            title = ctx.title_case_name,
            project = ctx.project_name,
        );
        assert_eq!(manifest, expected);

        let _ = fs::remove_dir_all(&dest);
    }

    /// A `[deeplink]` scheme (+ optional host) renders the
    /// `android:launchMode="singleTop"` attribute and a
    /// VIEW/BROWSABLE/DEFAULT `<intent-filter>`.
    #[test]
    fn generate_android_manifest_with_deeplink_renders_intent_filter() {
        let dest = unique_temp_dir("manifest-with-deeplink");
        let mut ctx = test_context();
        ctx.deeplink_scheme = Some("myapp".into());
        ctx.deeplink_host = Some("open".into());

        generate(&dest, &ctx, None, false, None).unwrap();

        let manifest =
            fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            manifest.contains("android:launchMode=\"singleTop\""),
            "{manifest}"
        );
        assert!(
            manifest.contains("<action android:name=\"android.intent.action.VIEW\" />"),
            "{manifest}"
        );
        assert!(
            manifest.contains("<category android:name=\"android.intent.category.BROWSABLE\" />"),
            "{manifest}"
        );
        assert!(
            manifest.contains("<category android:name=\"android.intent.category.DEFAULT\" />"),
            "{manifest}"
        );
        assert!(
            manifest.contains("android:scheme=\"myapp\" android:host=\"open\""),
            "{manifest}"
        );
        // Well-formed XML: the same open/close tag count.
        assert_eq!(
            manifest.matches("<intent-filter>").count(),
            manifest.matches("</intent-filter>").count()
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// A scheme with no host omits `android:host` entirely (not an empty
    /// attribute).
    #[test]
    fn generate_android_manifest_with_deeplink_scheme_only_omits_host_attr() {
        let dest = unique_temp_dir("manifest-deeplink-no-host");
        let mut ctx = test_context();
        ctx.deeplink_scheme = Some("myapp".into());

        generate(&dest, &ctx, None, false, None).unwrap();

        let manifest =
            fs::read_to_string(dest.join("android/app/src/main/AndroidManifest.xml")).unwrap();
        assert!(
            manifest.contains("android:scheme=\"myapp\" />"),
            "{manifest}"
        );
        assert!(!manifest.contains("android:host"), "{manifest}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// With no `[deeplink]` config, the rendered Info.plist has no
    /// `CFBundleURLTypes` key and stays byte-identical to its
    /// pre-deep-link-support rendering.
    #[test]
    fn generate_info_plist_without_deeplink_is_byte_identical_to_pre_task_baseline() {
        let dest = unique_temp_dir("plist-no-deeplink");
        let ctx = test_context();
        assert!(ctx.deeplink_scheme.is_none());

        generate(&dest, &ctx, None, false, None).unwrap();

        let plist = fs::read_to_string(dest.join("ios/Runner/Info.plist")).unwrap();
        assert!(!plist.contains("CFBundleURLTypes"), "{plist}");
        let expected_tail = "\t<key>UISupportedInterfaceOrientations~ipad</key>\n\
             \t<array>\n\
             \t\t<string>UIInterfaceOrientationPortrait</string>\n\
             \t\t<string>UIInterfaceOrientationPortraitUpsideDown</string>\n\
             \t\t<string>UIInterfaceOrientationLandscapeLeft</string>\n\
             \t\t<string>UIInterfaceOrientationLandscapeRight</string>\n\
             \t</array>\n\
             \t<!-- Enable 120Hz on ProMotion iPhone hardware. This key is REQUIRED for\n\
             \t     >60fps refresh rates on iPhone Pro models (Apple Foundation reference:\n\
             \t     CADisplayLink preferredFrameRateRange, accessed 2026-07-18). Without it,\n\
             \t     the system silently caps at 60Hz even if preferredFrameRateRange requests\n\
             \t     higher. -->\n\
             \t<key>CADisableMinimumFrameDurationOnPhone</key>\n\
             \t<true/>\n\
             </dict>\n\
             </plist>\n";
        assert!(
            plist.ends_with(expected_tail),
            "expected plist to end with:\n{expected_tail}\ngot:\n{plist}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// A `[deeplink]` scheme renders a valid `CFBundleURLTypes` entry with
    /// the scheme and the derived iOS bundle id as `CFBundleURLName`.
    #[test]
    fn generate_info_plist_with_deeplink_renders_cfbundle_url_types() {
        let dest = unique_temp_dir("plist-with-deeplink");
        let mut ctx = test_context();
        ctx.deeplink_scheme = Some("myapp".into());

        generate(&dest, &ctx, None, false, None).unwrap();

        let plist = fs::read_to_string(dest.join("ios/Runner/Info.plist")).unwrap();
        assert!(plist.contains("<key>CFBundleURLTypes</key>"), "{plist}");
        assert!(plist.contains("<string>myapp</string>"), "{plist}");
        assert!(
            plist.contains(&format!("<string>{}</string>", ctx.ios_identifier())),
            "{plist}"
        );
        // Well-formed plist tail: closes cleanly.
        assert!(plist.trim_end().ends_with("</plist>"), "{plist}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// `frust.toml` records the `[deeplink]` config when set, and omits the
    /// section entirely when not.
    #[test]
    fn generate_frust_toml_records_deeplink_section_when_configured() {
        let dest = unique_temp_dir("toml-deeplink");
        let mut ctx = test_context();
        ctx.deeplink_scheme = Some("myapp".into());
        ctx.deeplink_host = Some("open".into());

        generate(&dest, &ctx, None, false, None).unwrap();

        let toml = fs::read_to_string(dest.join("frust.toml")).unwrap();
        assert!(toml.contains("[deeplink]"), "{toml}");
        assert!(toml.contains("scheme = \"myapp\""), "{toml}");
        assert!(toml.contains("host = \"open\""), "{toml}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_frust_toml_omits_deeplink_section_by_default() {
        let dest = unique_temp_dir("toml-no-deeplink");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

        let toml = fs::read_to_string(dest.join("frust.toml")).unwrap();
        assert!(!toml.contains("[deeplink]"), "{toml}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_rejects_org_producing_invalid_ios_bundle_id() {
        let dest = unique_temp_dir("ios-invalid-id");
        let mut ctx = test_context();
        // Whitespace in `org` derives a bundle id with an invalid segment.
        ctx.org = "dev f0x".into();

        let err = generate(&dest, &ctx, None, false, None).unwrap_err();
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

        generate(&dest, &ctx, None, false, None).unwrap();

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

        generate(&dest, &ctx, None, false, None).unwrap();

        let jar_bytes = fs::read(dest.join("android/gradle/wrapper/gradle-wrapper.jar")).unwrap();
        // Zip local file header magic — confirms the binary content
        // survived the embed → generate round-trip byte-for-byte.
        assert_eq!(&jar_bytes[0..4], b"PK\x03\x04", "not a valid zip/jar");

        let _ = fs::remove_dir_all(&dest);
    }

    /// `arch = None` must render `Cargo.toml` and `src/lib.rs`
    /// byte-identically to rendering the default template files directly —
    /// proof the arch-tag machinery (`split_arch_tag`, the manifest-loop
    /// override check) never engages on the default path, independent of
    /// any test that merely spot-checks substrings.
    #[test]
    fn generate_with_no_arch_renders_default_template_files_byte_identically() {
        let dest = unique_temp_dir("no-arch-byte-identical");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

        let repo_templates = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/app");
        let mut render_vars = ctx.render_vars();
        render_vars.extend(context::platform_render_vars(ScaffoldPlatform::DEFAULT));
        for (raw_name, out_name) in [
            ("Cargo.toml.tmpl", "Cargo.toml"),
            ("src/lib.rs.tmpl", "src/lib.rs"),
        ] {
            let raw = fs::read_to_string(repo_templates.join(raw_name))
                .unwrap_or_else(|e| panic!("reading {raw_name}: {e}"));
            let expected = renderer::render(&raw, &render_vars).unwrap();
            let actual = fs::read_to_string(dest.join(out_name))
                .unwrap_or_else(|e| panic!("reading generated {out_name}: {e}"));
            assert_eq!(
                actual, expected,
                "generate(arch=None) must render `{out_name}` byte-identically to the \
                 default `{raw_name}` template"
            );
        }

        let _ = fs::remove_dir_all(&dest);
    }

    /// An unrecognized `--arch` value is rejected before any file is
    /// written.
    #[test]
    fn generate_rejects_unknown_arch_before_writing_any_file() {
        let dest = unique_temp_dir("unknown-arch");
        let ctx = test_context();

        let err = generate(&dest, &ctx, None, false, Some("not-a-real-arch")).unwrap_err();
        assert!(err.to_string().contains("not-a-real-arch"), "{err}");
        assert!(!dest.join("Cargo.toml").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    /// `--arch clean-signals` renders the variant `Cargo.toml`/full
    /// `counter` feature-slice tree in place of the default template's
    /// counter screen, while every arch-agnostic file (frust.toml, assets, android/ios
    /// trees) still lands exactly once.
    #[test]
    fn generate_with_clean_signals_arch_renders_variant_content_in_place_of_defaults() {
        let dest = unique_temp_dir("clean-signals-arch");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false, Some("clean-signals")).unwrap();

        // Exactly one `Cargo.toml`/`src/lib.rs` entry each — the base and
        // variant manifest entries never both land.
        assert_eq!(
            written
                .iter()
                .filter(|p| p == &Path::new("Cargo.toml"))
                .count(),
            1
        );
        assert_eq!(
            written
                .iter()
                .filter(|p| p == &Path::new("src/lib.rs"))
                .count(),
            1
        );

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains(&ctx.project_name), "{cargo_toml}");
        assert!(
            cargo_toml.contains("clean-signals-frust = { path ="),
            "{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("clean-signals = \"0.1\""),
            "{cargo_toml}"
        );
        // The notes-app demo's plugin dependency doesn't apply to this
        // variant.
        assert!(
            !cargo_toml.contains("frust-shared-preferences = {"),
            "{cargo_toml}"
        );
        // Material 3 is this variant's design system too.
        assert!(
            cargo_toml
                .contains("frust-material = { path = \"/path/to/frust/../../plugins/material\" }"),
            "{cargo_toml}"
        );
        assert!(!cargo_toml.contains("frust-glyph"), "{cargo_toml}");

        // The full `counter` feature-slice tree lands.
        let expected_files = [
            "src/lib.rs",
            "src/failure.rs",
            "src/features/mod.rs",
            "src/features/counter/mod.rs",
            "src/features/counter/domain/mod.rs",
            "src/features/counter/domain/repositories.rs",
            "src/features/counter/domain/use_cases/mod.rs",
            "src/features/counter/domain/use_cases/load_count.rs",
            "src/features/counter/domain/use_cases/increment_count.rs",
            "src/features/counter/data/mod.rs",
            "src/features/counter/data/sources.rs",
            "src/features/counter/data/repositories.rs",
            "src/features/counter/presentation/mod.rs",
            "src/features/counter/presentation/controllers.rs",
            "src/features/counter/presentation/pages.rs",
        ];
        for f in expected_files {
            assert!(
                written.iter().any(|p| p == Path::new(f)),
                "expected `{f}` in written paths: {written:?}"
            );
            assert!(dest.join(f).exists(), "expected `{f}` to exist on disk");
        }

        // lib.rs shrinks to module decls + `app!` wiring + the composition
        // root — no `CounterController` construction/`use_controller`/`async_view`
        // details leak into it anymore (those moved into the feature
        // slice).
        let lib_rs = fs::read_to_string(dest.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("pub mod failure;"), "{lib_rs}");
        assert!(lib_rs.contains("pub mod features;"), "{lib_rs}");
        assert!(lib_rs.contains("impl Component for MyAppApp"), "{lib_rs}");
        // Material-by-default applies to this variant too — see the matching
        // assertion in `generate_produces_manifest_listed_files_with_substitutions`.
        assert!(
            lib_rs.contains(
                "frust::app!(\n    MyAppApp,\n    setup = {\n        frust_material::install();\n    }\n);"
            ),
            "{lib_rs}"
        );
        assert!(
            lib_rs.contains("type State = Arc<CounterController>;"),
            "{lib_rs}"
        );
        assert!(lib_rs.contains("pages::counter_page(state)"), "{lib_rs}");
        assert!(!lib_rs.contains("app_logic"), "{lib_rs}");
        assert!(!lib_rs.contains("frust-glyph"), "{lib_rs}");
        assert!(
            !lib_rs.contains("        frust_glyph::install();"),
            "{lib_rs}"
        );
        // The controller's construction/rendering detail moved into the
        // feature slice; lib.rs only names the type as its `State`.
        assert!(!lib_rs.contains("ControllerCore"), "{lib_rs}");
        assert!(!lib_rs.contains("use_controller"), "{lib_rs}");
        assert!(!lib_rs.contains("async_view"), "{lib_rs}");
        // The notes-app demo's own shape doesn't leak into this variant.
        assert!(!lib_rs.contains("SharedPreferences"), "{lib_rs}");
        assert!(!lib_rs.contains("text_input("), "{lib_rs}");

        // The moved pieces land in their expected layer files.
        let failure_rs = fs::read_to_string(dest.join("src/failure.rs")).unwrap();
        assert!(failure_rs.contains("pub enum AppFailure"), "{failure_rs}");
        assert!(failure_rs.contains("Network(String)"), "{failure_rs}");
        assert!(failure_rs.contains("Validation(String)"), "{failure_rs}");

        let domain_repo =
            fs::read_to_string(dest.join("src/features/counter/domain/repositories.rs")).unwrap();
        assert!(
            domain_repo.contains("pub trait CounterRepository"),
            "{domain_repo}"
        );

        let load_count =
            fs::read_to_string(dest.join("src/features/counter/domain/use_cases/load_count.rs"))
                .unwrap();
        assert!(load_count.contains("pub struct LoadCount"), "{load_count}");
        assert!(load_count.contains("NoParams"), "{load_count}");
        let increment_count = fs::read_to_string(
            dest.join("src/features/counter/domain/use_cases/increment_count.rs"),
        )
        .unwrap();
        assert!(
            increment_count.contains("pub struct IncrementCount"),
            "{increment_count}"
        );

        let data_repo =
            fs::read_to_string(dest.join("src/features/counter/data/repositories.rs")).unwrap();
        assert!(
            data_repo.contains("pub struct InMemoryCounterRepository"),
            "{data_repo}"
        );
        assert!(data_repo.contains("fn map_source_error"), "{data_repo}");

        let controllers =
            fs::read_to_string(dest.join("src/features/counter/presentation/controllers.rs"))
                .unwrap();
        assert!(
            controllers.contains("pub struct CounterController"),
            "{controllers}"
        );

        // The page is a Material scaffold built from small widget fns.
        let pages =
            fs::read_to_string(dest.join("src/features/counter/presentation/pages.rs")).unwrap();
        assert!(pages.contains("use_controller"), "{pages}");
        assert!(pages.contains("async_view"), "{pages}");
        assert!(
            pages.contains("pub fn counter_page")
                && pages.contains("scaffold(")
                && pages.contains("app_bar")
                && pages.contains("fab(")
                && pages.contains("icons::ADD"),
            "{pages}"
        );
        assert!(!pages.contains("frust_glyph"), "{pages}");

        // No `.clean-signals.` leftover in any written path, and no stray
        // `Cargo.toml.clean-signals`/`src/lib.rs.clean-signals` files.
        assert!(!dest.join("Cargo.toml.clean-signals").exists());
        // The default template's counter screen is not part of this arch.
        assert!(!dest.join("src/home_page.rs").exists());
        assert!(!dest.join("src/lib.rs.clean-signals").exists());
        assert!(
            written
                .iter()
                .all(|p| !p.to_string_lossy().contains(".clean-signals.")),
            "{written:?}"
        );

        // Arch-agnostic files still land untouched.
        assert!(dest.join("frust.toml").exists());
        assert!(dest.join("assets/logo.png").exists());
        assert!(dest.join("android").is_dir());
        assert!(dest.join("ios").is_dir());
        assert!(dest.join("macos").is_dir());
        assert!(dest.join("windows").is_dir());
        assert!(dest.join("linux").is_dir());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_desktop_tree_dir_names_with_stripped_tmpl_suffix() {
        let dest = unique_temp_dir("desktop-dir-suffix");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

        // `macos.tmpl/`, `windows.tmpl/`, `linux.tmpl/` -> `macos/`,
        // `windows/`, `linux/`: the marker suffix on the source directory
        // doesn't leak into the generated project, mirroring
        // `android.tmpl/`/`ios.tmpl/`.
        for (rendered, marker) in [
            ("macos", "macos.tmpl"),
            ("windows", "windows.tmpl"),
            ("linux", "linux.tmpl"),
        ] {
            assert!(dest.join(rendered).is_dir(), "expected `{rendered}/`");
            assert!(
                !dest.join(marker).exists(),
                "`{marker}/` marker suffix must not survive rendering"
            );
        }

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_macos_info_plist_and_entitlements_with_substitutions() {
        let dest = unique_temp_dir("macos-tree");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false, None).unwrap();
        for f in ["macos/Info.plist", "macos/app.entitlements"] {
            assert!(
                written.iter().any(|p| p == Path::new(f)),
                "expected `{f}` in written paths: {written:?}"
            );
        }

        let plist = fs::read_to_string(dest.join("macos/Info.plist")).unwrap();
        assert!(
            plist.contains(&format!("<string>{}</string>", ctx.title_case_name)),
            "{plist}"
        );
        assert!(
            plist.contains(&format!("<string>{}</string>", ctx.project_name)),
            "{plist}"
        );
        assert!(
            plist.contains(&format!("<string>{}</string>", ctx.desktop_identifier())),
            "{plist}"
        );
        assert!(
            plist.contains("<key>LSMinimumSystemVersion</key>\n\t<string>11.0</string>"),
            "{plist}"
        );
        assert!(!plist.contains("{{"), "{plist}");

        let entitlements = fs::read_to_string(dest.join("macos/app.entitlements")).unwrap();
        assert!(
            entitlements.contains(&ctx.title_case_name),
            "{entitlements}"
        );
        assert!(!entitlements.contains("{{"), "{entitlements}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// With no `[deeplink]` config, the rendered macOS Info.plist has no
    /// `CFBundleURLTypes` key, mirroring the iOS baseline test.
    #[test]
    fn generate_macos_info_plist_without_deeplink_omits_cfbundle_url_types() {
        let dest = unique_temp_dir("macos-plist-no-deeplink");
        let ctx = test_context();
        assert!(ctx.deeplink_scheme.is_none());

        generate(&dest, &ctx, None, false, None).unwrap();

        let plist = fs::read_to_string(dest.join("macos/Info.plist")).unwrap();
        assert!(!plist.contains("CFBundleURLTypes"), "{plist}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// A `[deeplink]` scheme renders a `CFBundleURLTypes` entry in the macOS
    /// Info.plist too, not just the iOS one.
    #[test]
    fn generate_macos_info_plist_with_deeplink_renders_cfbundle_url_types() {
        let dest = unique_temp_dir("macos-plist-with-deeplink");
        let mut ctx = test_context();
        ctx.deeplink_scheme = Some("myapp".into());

        generate(&dest, &ctx, None, false, None).unwrap();

        let plist = fs::read_to_string(dest.join("macos/Info.plist")).unwrap();
        assert!(plist.contains("<key>CFBundleURLTypes</key>"), "{plist}");
        assert!(plist.contains("<string>myapp</string>"), "{plist}");
        assert!(
            plist.contains(&format!("<string>{}</string>", ctx.desktop_identifier())),
            "{plist}"
        );
        assert!(plist.trim_end().ends_with("</plist>"), "{plist}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_windows_build_rs_and_wires_cargo_toml() {
        let dest = unique_temp_dir("windows-tree");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false, None).unwrap();
        assert!(
            written.iter().any(|p| p == Path::new("windows/build.rs")),
            "expected `windows/build.rs` in written paths: {written:?}"
        );

        let build_rs = fs::read_to_string(dest.join("windows/build.rs")).unwrap();
        assert!(
            build_rs.contains("winresource::WindowsResource::new()"),
            "{build_rs}"
        );
        // Pinned to the full BuildLayout-derived path — a bare
        // `.contains("windows/icon.ico")` would also match a stale
        // pre-migration literal (e.g. a wrongly reintroduced root-level
        // `windows/icon.ico`), silently passing on the wrong path.
        // The generated `windows/build.rs` embeds this path as a portable,
        // forward-slash literal (it is a plain Rust string in the template,
        // not rendered from `BuildLayout` at scaffold time) — so the pin
        // below must compare the same portable form rather than
        // `BuildLayout::windows_icon()`'s own `PathBuf`, which renders with
        // the host's native separator (backslash on Windows).
        let icon_path = crate::host_path::to_portable_string(&BuildLayout::windows_icon());
        let icon_path = icon_path.as_str();
        assert_eq!(icon_path, "build/desktop/windows/icon.ico");
        assert!(
            build_rs.contains(icon_path),
            "windows/build.rs must reference `{icon_path}` \
             (BuildLayout::windows_icon()):\n{build_rs}"
        );
        assert!(build_rs.contains(&ctx.title_case_name), "{build_rs}");
        assert!(!build_rs.contains("{{"), "{build_rs}");

        // Cargo.toml wires the build script and the Windows-only
        // build-dependency the script needs.
        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(
            cargo_toml.contains("build = \"windows/build.rs\""),
            "{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("[target.'cfg(windows)'.build-dependencies]"),
            "{cargo_toml}"
        );
        assert!(cargo_toml.contains("winresource = \"0.1\""), "{cargo_toml}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// Both `Cargo.toml.tmpl` variants (default and `--arch clean-signals`)
    /// must carry the identical Windows build-script wiring — the
    /// clean-signals-guardrail precedent (CLAUDE.md) applied to this task's
    /// own addition.
    #[test]
    fn generate_wires_windows_build_script_identically_in_both_cargo_toml_variants() {
        let ctx = test_context();

        let default_dest = unique_temp_dir("windows-wiring-default");
        generate(&default_dest, &ctx, None, false, None).unwrap();
        let clean_signals_dest = unique_temp_dir("windows-wiring-clean-signals");
        generate(
            &clean_signals_dest,
            &ctx,
            None,
            false,
            Some("clean-signals"),
        )
        .unwrap();

        for dest in [&default_dest, &clean_signals_dest] {
            let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
            assert!(
                cargo_toml.contains("build = \"windows/build.rs\""),
                "{cargo_toml}"
            );
            assert!(
                cargo_toml.contains("[target.'cfg(windows)'.build-dependencies]"),
                "{cargo_toml}"
            );
            assert!(cargo_toml.contains("winresource = \"0.1\""), "{cargo_toml}");
        }

        let _ = fs::remove_dir_all(&default_dest);
        let _ = fs::remove_dir_all(&clean_signals_dest);
    }

    /// The browser target's one dependency row, in BOTH `Cargo.toml`
    /// variants: `frust::app!` expands to `frust::web_app!` on `wasm32`, and
    /// that expansion carries a `#[wasm_bindgen(start)]` attribute whose own
    /// generated glue names `wasm_bindgen::` paths inside the app crate — so
    /// the app crate needs the dependency edge itself (without it the
    /// scaffold fails `E0433: cannot find module or crate `wasm_bindgen``).
    ///
    /// The pin is asserted EXACT, not merely present: `wasm-bindgen`'s crate
    /// and CLI share a schema version, so `=0.2.128` is a contract with the
    /// host toolchain rather than a semver floor (Version-Pin Policy).
    #[test]
    fn both_cargo_toml_variants_carry_the_exact_wasm32_wasm_bindgen_pin() {
        let ctx = test_context();

        let default_dest = unique_temp_dir("wasm-row-default");
        generate(&default_dest, &ctx, None, false, None).unwrap();
        let clean_signals_dest = unique_temp_dir("wasm-row-clean-signals");
        generate(
            &clean_signals_dest,
            &ctx,
            None,
            false,
            Some("clean-signals"),
        )
        .unwrap();

        for dest in [&default_dest, &clean_signals_dest] {
            let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
            assert!(
                cargo_toml.contains("[target.'cfg(target_arch = \"wasm32\")'.dependencies]"),
                "{cargo_toml}"
            );
            assert!(
                cargo_toml.contains("wasm-bindgen = \"=0.2.128\""),
                "{cargo_toml}"
            );
            // Target-gated, never an unconditional row: a native build must
            // not resolve it.
            assert!(
                !cargo_toml.contains("\nwasm-bindgen = \"=0.2.128\"\n\n[dependencies]"),
                "{cargo_toml}"
            );
        }

        let _ = fs::remove_dir_all(&default_dest);
        let _ = fs::remove_dir_all(&clean_signals_dest);
    }

    /// `src/main.rs`'s target gating, which is what makes a scaffold
    /// buildable for the browser at all.
    ///
    /// Two independent facts, both load-bearing:
    ///
    /// 1. The desktop `main` is gated off `wasm32` as well as Android —
    ///    `frust::app!` emits no `__frust_main` on either, so calling it
    ///    there is `E0425`.
    /// 2. The `wasm32` `main` is NOT empty. On that target this crate's
    ///    `cdylib` and its bin compile to the same
    ///    `<profile>/<name>.wasm` — cargo reports an "output filename
    ///    collision" and writes one of them, in an order nothing downstream
    ///    can choose. Binding the library's browser entry keeps the whole
    ///    library in the bin's link graph, so BOTH candidates are complete,
    ///    startable modules; an empty `main` there ships a few-kilobyte stub
    ///    that instantiates and silently mounts nothing.
    #[test]
    fn generate_main_rs_gates_the_desktop_entry_off_wasm32_and_keeps_the_lib_linked() {
        let dest = unique_temp_dir("main-rs-wasm-gate");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

        let main_rs = fs::read_to_string(dest.join("src/main.rs")).unwrap();
        assert!(
            main_rs.contains("#[cfg(not(any(target_os = \"android\", target_arch = \"wasm32\")))]"),
            "{main_rs}"
        );
        assert!(
            main_rs.contains(&format!("{}::__frust_main();", ctx.project_name)),
            "{main_rs}"
        );
        assert!(
            main_rs.contains("#[cfg(target_arch = \"wasm32\")]"),
            "{main_rs}"
        );
        assert!(
            main_rs.contains(&format!(
                "let _entry: fn() = {}::__frust_web_start;",
                ctx.project_name
            )),
            "the wasm32 `main` must keep the library's browser entry in the \
             binary's link graph:\n{main_rs}"
        );
        assert!(!main_rs.contains("{{"), "{main_rs}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_produces_linux_desktop_entry_with_substitutions() {
        let dest = unique_temp_dir("linux-tree");
        let ctx = test_context();

        let written = generate(&dest, &ctx, None, false, None).unwrap();
        assert!(
            written.iter().any(|p| p == Path::new("linux/app.desktop")),
            "expected `linux/app.desktop` in written paths: {written:?}"
        );

        let desktop_entry = fs::read_to_string(dest.join("linux/app.desktop")).unwrap();
        assert!(
            desktop_entry.starts_with("[Desktop Entry]"),
            "{desktop_entry}"
        );
        assert!(
            desktop_entry.contains(&format!("Name={}", ctx.title_case_name)),
            "{desktop_entry}"
        );
        assert!(
            desktop_entry.contains(&format!("Exec={}", ctx.project_name)),
            "{desktop_entry}"
        );
        assert!(
            desktop_entry.contains(&format!("Icon={}", ctx.desktop_identifier())),
            "{desktop_entry}"
        );
        assert!(
            desktop_entry.contains("Categories=Utility;"),
            "{desktop_entry}"
        );
        assert!(!desktop_entry.contains("{{"), "{desktop_entry}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_frust_toml_includes_populated_desktop_section_and_commented_platform_stubs() {
        let dest = unique_temp_dir("toml-desktop-section");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();

        let toml = fs::read_to_string(dest.join("frust.toml")).unwrap();
        assert!(toml.contains("[desktop]"), "{toml}");
        assert!(
            toml.contains(&format!("name = \"{}\"", ctx.title_case_name)),
            "{toml}"
        );
        assert!(
            toml.contains(&format!("identifier = \"{}\"", ctx.desktop_identifier())),
            "{toml}"
        );
        assert!(toml.contains("icon = \"assets/logo.png\""), "{toml}");
        // The per-platform stubs stay commented out by default.
        assert!(toml.contains("# [macos]"), "{toml}");
        assert!(toml.contains("# [windows]"), "{toml}");
        assert!(toml.contains("# [linux]"), "{toml}");
        // The browser stub is the fourth platform's, and documents every
        // `manifest::WebSection` key at its own default.
        assert!(toml.contains("# [web]"), "{toml}");
        for key in [
            "# host-dir = \"web\"",
            "# out-dir = \"build/web\"",
            "# wasm-opt = true",
            "# port = 8000",
        ] {
            assert!(toml.contains(key), "missing `{key}` in:\n{toml}");
        }
        assert!(
            toml.contains(&format!("# out-name = \"{}\"", ctx.project_name)),
            "{toml}"
        );
        assert!(!toml.contains("{{"), "{toml}");

        // Commented out means *inert*: the scaffolded manifest still parses
        // and still carries no `[web]` section, so a browser build runs on
        // the documented defaults rather than on whatever the stub spells.
        let parsed = crate::manifest::load_optional(&dest)
            .unwrap()
            .expect("the scaffolded frust.toml must parse");
        assert!(
            parsed.web.is_none(),
            "the `[web]` stub must stay commented out: {:?}",
            parsed.web
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// Re-running `generate` with `overwrite` against its own prior output
    /// reproduces every new desktop-template file byte-for-byte — the
    /// idempotency half of the scaffold's contract (byte-identical output,
    /// never a silent partial write).
    #[test]
    fn generate_desktop_templates_are_idempotent_under_overwrite() {
        let dest = unique_temp_dir("desktop-idempotent");
        let ctx = test_context();

        generate(&dest, &ctx, None, false, None).unwrap();
        let first: Vec<(PathBuf, String)> = [
            "macos/Info.plist",
            "macos/app.entitlements",
            "windows/build.rs",
            "linux/app.desktop",
            "frust.toml",
            "Cargo.toml",
        ]
        .iter()
        .map(|f| (dest.join(f), fs::read_to_string(dest.join(f)).unwrap()))
        .collect();

        generate(&dest, &ctx, None, true, None).unwrap();

        for (path, before) in first {
            let after = fs::read_to_string(&path).unwrap();
            assert_eq!(
                before,
                after,
                "`{}` must render byte-identically on a repeat `--overwrite` generate",
                path.display()
            );
        }

        let _ = fs::remove_dir_all(&dest);
    }

    // ---- platform-inclusion axis -------------------------------------

    /// The repository root, resolved `CARGO_MANIFEST_DIR`-relative — used by
    /// the drift tests below to read the `crates/frust-shell-web/platform/web` embedder these
    /// templates are derived from.
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Every file under `root`, as `(relative path, bytes)`, sorted by path.
    fn tree_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            let mut entries: Vec<_> = fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    let relative = path
                        .strip_prefix(root)
                        .expect("walked path is under root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((relative, fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn platform_tags_round_trip_and_cover_every_variant() {
        for platform in ScaffoldPlatform::ALL {
            assert_eq!(ScaffoldPlatform::from_tag(platform.tag()), Some(*platform));
            assert_eq!(platform.template_dir(), format!("{}.tmpl", platform.tag()));
        }
        assert_eq!(ScaffoldPlatform::from_tag("solaris"), None);
        assert_eq!(
            known_platform_tags(),
            vec!["android", "ios", "macos", "windows", "linux", "web"]
        );
        // The default set is exactly the pre-axis five: every platform
        // except the browser target.
        assert_eq!(
            ScaffoldPlatform::DEFAULT.len(),
            ScaffoldPlatform::ALL.len() - 1
        );
        assert!(!ScaffoldPlatform::DEFAULT.contains(&ScaffoldPlatform::Web));
        for platform in ScaffoldPlatform::DEFAULT {
            assert!(ScaffoldPlatform::ALL.contains(platform));
        }
    }

    /// The platform check is anchored at the entry's FIRST path component:
    /// a platform-agnostic entry is never captured, and neither is a nested
    /// directory that happens to share a platform's name.
    #[test]
    fn split_platform_dir_matches_only_a_leading_platform_marker_directory() {
        assert_eq!(
            split_platform_dir("web.tmpl/index.html"),
            Some((ScaffoldPlatform::Web, "index.html"))
        );
        assert_eq!(
            split_platform_dir("android.tmpl/app/build.gradle.kts"),
            Some((ScaffoldPlatform::Android, "app/build.gradle.kts"))
        );
        for agnostic in [
            "Cargo.toml",
            "src/lib.rs",
            "assets/logo.png",
            "web/index.html",
            "src/web.tmpl/thing.rs",
        ] {
            assert_eq!(split_platform_dir(agnostic), None, "{agnostic}");
        }
    }

    /// The golden test the axis is judged on: excluding the browser target
    /// must leave today's scaffold **byte-identical**, and including it must
    /// add the web tree and NOTHING else — not a changed byte anywhere in
    /// the other 63 files, not a reordering, not an extra directory.
    #[test]
    fn including_web_adds_exactly_the_web_tree_and_changes_no_other_file() {
        let without = unique_temp_dir("platforms-without-web");
        let with = unique_temp_dir("platforms-with-web");
        let ctx = test_context();

        let default_written = generate(&without, &ctx, None, false, None).unwrap();
        let all_written =
            generate_with_platforms(&with, &ctx, None, false, None, ScaffoldPlatform::ALL).unwrap();

        let before = tree_snapshot(&without);
        let after = tree_snapshot(&with);

        // Path sets differ by exactly the two web files.
        let before_paths: Vec<&str> = before.iter().map(|(p, _)| p.as_str()).collect();
        let after_paths: Vec<&str> = after.iter().map(|(p, _)| p.as_str()).collect();
        let added: Vec<&str> = after_paths
            .iter()
            .copied()
            .filter(|p| !before_paths.contains(p))
            .collect();
        let removed: Vec<&str> = before_paths
            .iter()
            .copied()
            .filter(|p| !after_paths.contains(p))
            .collect();
        assert_eq!(added, vec!["web/frust_web.js", "web/index.html"]);
        assert!(removed.is_empty(), "excluding web removed {removed:?}");

        // Every shared file is byte-identical.
        for (path, bytes) in &before {
            let (_, other) = after
                .iter()
                .find(|(p, _)| p == path)
                .unwrap_or_else(|| panic!("`{path}` missing from the with-web scaffold"));
            assert_eq!(
                bytes, other,
                "`{path}` differs between the two platform sets"
            );
        }

        // And the returned path lists agree the same way (manifest order,
        // web appended where its manifest rows sit).
        assert_eq!(all_written.len(), default_written.len() + 2);
        for written in &default_written {
            assert!(
                all_written.contains(written),
                "`{}` dropped from the with-web run",
                written.display()
            );
        }

        let _ = fs::remove_dir_all(&without);
        let _ = fs::remove_dir_all(&with);
    }

    /// `generate` is defined as `generate_with_platforms(.., DEFAULT)`;
    /// this pins that the two really do produce the same tree, so the
    /// wrapper can never drift from the set it claims to pass.
    #[test]
    fn generate_equals_generate_with_platforms_over_the_default_set() {
        let via_wrapper = unique_temp_dir("platforms-wrapper");
        let via_explicit = unique_temp_dir("platforms-explicit");
        let ctx = test_context();

        generate(&via_wrapper, &ctx, None, false, None).unwrap();
        generate_with_platforms(
            &via_explicit,
            &ctx,
            None,
            false,
            None,
            ScaffoldPlatform::DEFAULT,
        )
        .unwrap();

        assert_eq!(tree_snapshot(&via_wrapper), tree_snapshot(&via_explicit));

        let _ = fs::remove_dir_all(&via_wrapper);
        let _ = fs::remove_dir_all(&via_explicit);
    }

    /// A de-selected platform's whole subtree disappears; the
    /// platform-agnostic files (crate manifest, `frust.toml`, `src/`,
    /// assets) are never affected by the axis.
    #[test]
    fn generate_with_platforms_emits_only_the_selected_platform_trees() {
        let dest = unique_temp_dir("platforms-web-only");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, &[ScaffoldPlatform::Web]).unwrap();

        assert!(dest.join("web/index.html").is_file());
        assert!(dest.join("web/frust_web.js").is_file());
        for absent in ["android", "ios", "macos", "windows", "linux"] {
            assert!(
                !dest.join(absent).exists(),
                "`{absent}/` must not be emitted"
            );
        }
        // Platform-agnostic entries are untouched by the axis.
        for present in ["Cargo.toml", "frust.toml", "src/lib.rs", "assets/logo.png"] {
            assert!(
                dest.join(present).exists(),
                "`{present}` must still be emitted"
            );
        }

        let _ = fs::remove_dir_all(&dest);
    }

    /// An empty platform set is a valid request, not an error: a plain app
    /// crate with no platform project at all.
    #[test]
    fn generate_with_no_platforms_still_emits_the_app_crate() {
        let dest = unique_temp_dir("platforms-none");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, &[]).unwrap();

        assert!(dest.join("Cargo.toml").is_file());
        assert!(dest.join("src/lib.rs").is_file());
        for absent in ["android", "ios", "macos", "windows", "linux", "web"] {
            assert!(
                !dest.join(absent).exists(),
                "`{absent}/` must not be emitted"
            );
        }

        let _ = fs::remove_dir_all(&dest);
    }

    /// `windows/build.rs` is only ever a valid `build` script when the
    /// `windows.tmpl/` subtree is actually emitted — a selection that
    /// excludes it must not still
    /// wire the manifest's `build =` key or the `cfg(windows)`
    /// build-dependency table, or `cargo build` fails immediately trying to
    /// read a file that was never written.
    #[test]
    fn generate_with_platforms_web_only_omits_windows_build_wiring() {
        let dest = unique_temp_dir("platforms-web-only-no-windows-wiring");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, &[ScaffoldPlatform::Web]).unwrap();

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(
            !cargo_toml.contains("build = \"windows/build.rs\""),
            "{cargo_toml}"
        );
        assert!(
            !cargo_toml.contains("[target.'cfg(windows)'.build-dependencies]"),
            "{cargo_toml}"
        );
        assert!(!cargo_toml.contains("winresource"), "{cargo_toml}");
        assert!(!cargo_toml.contains("{{"), "{cargo_toml}");
        assert!(!dest.join("windows").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    /// The counterpart of the test above: selecting every platform (not
    /// just the pre-axis [`ScaffoldPlatform::DEFAULT`] set the byte-identity
    /// golden already covers) still wires both pieces.
    #[test]
    fn generate_with_platforms_all_still_wires_windows_build() {
        let dest = unique_temp_dir("platforms-all-windows-wiring");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, ScaffoldPlatform::ALL).unwrap();

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(
            cargo_toml.contains("build = \"windows/build.rs\""),
            "{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("[target.'cfg(windows)'.build-dependencies]"),
            "{cargo_toml}"
        );
        assert!(cargo_toml.contains("winresource = \"0.1\""), "{cargo_toml}");

        let _ = fs::remove_dir_all(&dest);
    }

    /// An empty platform selection — [`generate_with_no_platforms_still_emits_the_app_crate`]'s
    /// case — must not leave the windows wiring behind either: the manifest
    /// this produces is the one [`generate_with_platforms`]'s own doc
    /// promises is "a valid app crate", which a stray `build =
    /// "windows/build.rs"` key would silently break for every host.
    #[test]
    fn generate_with_no_platforms_cargo_toml_carries_neither_windows_wiring_key() {
        let dest = unique_temp_dir("platforms-none-no-windows-wiring");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, &[]).unwrap();

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(
            !cargo_toml.contains("build = \"windows/build.rs\""),
            "{cargo_toml}"
        );
        assert!(
            !cargo_toml.contains("[target.'cfg(windows)'.build-dependencies]"),
            "{cargo_toml}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// Both `Cargo.toml.tmpl` variants must obey the platform axis
    /// identically — the same clean-signals-guardrail precedent
    /// [`generate_wires_windows_build_script_identically_in_both_cargo_toml_variants`]
    /// pins for the always-on case, applied to a windows-less selection.
    #[test]
    fn generate_with_platforms_web_only_omits_windows_wiring_in_both_cargo_toml_variants() {
        let ctx = test_context();

        let default_dest = unique_temp_dir("platforms-web-only-default");
        generate_with_platforms(
            &default_dest,
            &ctx,
            None,
            false,
            None,
            &[ScaffoldPlatform::Web],
        )
        .unwrap();
        let clean_signals_dest = unique_temp_dir("platforms-web-only-clean-signals");
        generate_with_platforms(
            &clean_signals_dest,
            &ctx,
            None,
            false,
            Some("clean-signals"),
            &[ScaffoldPlatform::Web],
        )
        .unwrap();

        for dest in [&default_dest, &clean_signals_dest] {
            let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
            assert!(
                !cargo_toml.contains("build = \"windows/build.rs\""),
                "{cargo_toml}"
            );
            assert!(
                !cargo_toml.contains("[target.'cfg(windows)'.build-dependencies]"),
                "{cargo_toml}"
            );
        }

        let _ = fs::remove_dir_all(&default_dest);
        let _ = fs::remove_dir_all(&clean_signals_dest);
    }

    /// Acceptance criterion (2): a windows-less manifest must actually
    /// RESOLVE, not merely fail two substring checks. `scaffold` is a pure
    /// file-write module with no `ProcessRunner` seam of its own (see
    /// `docs/CLI_ARCHITECTURE.md`'s scaffold row) and this crate's own real
    /// Cargo invocations are `frust-cli`'s ignored build gates, out of this
    /// task's declared write scope — so this resolves the manifest text with
    /// the `toml` crate (already a dependency, used by
    /// `context::manifest_names_package`) instead of shelling out to `cargo
    /// metadata`. LIMITATION: this proves the emitted manifest is
    /// well-formed TOML with no dangling `package.build` / `cfg(windows)`
    /// build-dependency table; it does not resolve the crate's dependency
    /// graph the way `cargo metadata`/`cargo build` would — that end-to-end
    /// proof is acceptance criterion (3)'s scratch build.
    #[test]
    fn generate_with_platforms_web_only_manifest_parses_with_no_windows_build_key() {
        let dest = unique_temp_dir("platforms-web-only-manifest-resolves");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, &[ScaffoldPlatform::Web]).unwrap();

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        // `toml::Table` (a keyed document), not `toml::Value::from_str`
        // (which parses a single bare value expression, not a whole
        // multi-table document, on this crate's pinned `1.1.4+spec-1.1.0`
        // line) — mirrors `context::manifest_names_package`'s existing
        // typed-deserialize precedent.
        let manifest: toml::Table =
            toml::from_str(&cargo_toml).expect("generated Cargo.toml must be valid TOML");

        let package = manifest
            .get("package")
            .and_then(|p| p.as_table())
            .expect("generated Cargo.toml must have a [package] table");
        assert!(
            !package.contains_key("build"),
            "a web-only selection must emit no `package.build` key: {package:?}"
        );

        let has_windows_build_deps = manifest
            .get("target")
            .and_then(|t| t.as_table())
            .and_then(|t| t.get("cfg(windows)"))
            .and_then(|t| t.as_table())
            .is_some_and(|t| t.contains_key("build-dependencies"));
        assert!(
            !has_windows_build_deps,
            "a web-only selection must emit no `[target.'cfg(windows)'.build-dependencies]` \
             table: {manifest:?}"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// The two axes are independent: selecting an arch does not resurrect a
    /// de-selected platform, and selecting a platform does not disturb arch
    /// resolution.
    #[test]
    fn the_platform_axis_and_the_arch_axis_do_not_interact() {
        let dest = unique_temp_dir("platforms-with-arch");
        let ctx = test_context();

        generate_with_platforms(
            &dest,
            &ctx,
            None,
            false,
            Some("clean-signals"),
            &[ScaffoldPlatform::Web],
        )
        .unwrap();

        // Arch resolution still happened...
        assert!(dest.join("src/features/counter/domain/mod.rs").is_file());
        // ...and the platform selection still holds.
        assert!(dest.join("web/index.html").is_file());
        assert!(!dest.join("android").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    // ---- web host page ------------------------------------------------

    #[test]
    fn generate_web_host_page_substitutes_the_title_and_module_name() {
        let dest = unique_temp_dir("web-host-page");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, ScaffoldPlatform::ALL).unwrap();

        let index = fs::read_to_string(dest.join("web/index.html")).unwrap();
        assert!(index.contains("<title>My App</title>"), "{index}");
        // The `?module=` default names this project's own wasm-bindgen
        // output, which is what `manifest::WebSection::out_name_or` falls
        // back to (`[app] name`) — the page and the build config agree
        // without either reading the other.
        assert!(index.contains("\"./pkg/my_app.js\""), "{index}");
        assert!(!index.contains("./pkg/app.js"), "{index}");
        // Strict-undefined rendering already guarantees this, but a
        // placeholder surviving into a generated page is the exact failure
        // an app author would hit first.
        assert!(!index.contains("{{"), "{index}");
        // The `.tmpl` marker never leaks into the emitted names.
        assert!(!dest.join("web.tmpl").exists());
        assert!(!dest.join("web/index.html.tmpl").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    /// Drift guard #1: the scaffolded `frust_web.js` is a byte-identical
    /// copy of `crates/frust-shell-web/platform/web/frust_web.js`. That embedder is the source of
    /// truth for the host-page contract; two copies of 300+ lines of glue
    /// silently diverging is the whole risk this duplication carries, and
    /// the glue is entirely app-agnostic so there is nothing to substitute.
    #[test]
    fn scaffolded_frust_web_js_is_byte_identical_to_the_platform_web_embedder() {
        let dest = unique_temp_dir("web-drift-js");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, ScaffoldPlatform::ALL).unwrap();

        let embedder = fs::read_to_string(
            repo_root().join("crates/frust-shell-web/platform/web/frust_web.js"),
        )
        .expect("reading crates/frust-shell-web/platform/web/frust_web.js");
        let scaffolded = fs::read_to_string(dest.join("web/frust_web.js")).unwrap();
        assert_eq!(
            scaffolded, embedder,
            "crates/frust-drive/templates/app/web.tmpl/frust_web.js.tmpl has drifted from \
             crates/frust-shell-web/platform/web/frust_web.js — re-copy it verbatim rather than \
             editing either copy alone"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// Drift guard #2: the scaffolded `index.html` differs from
    /// `crates/frust-shell-web/platform/web/index.html` in exactly three documented ways — its
    /// leading HTML comment (which addresses an app author rather than a
    /// framework reader), its `<title>`, and its `?module=` default. Every
    /// other byte — the whole `<style>` block that defines the canvas-host
    /// contract, and the module script that calls `mount()` — must match.
    #[test]
    fn scaffolded_index_html_differs_from_the_embedder_only_in_title_and_module_default() {
        /// Everything after the leading `<!-- ... -->` header comment.
        fn body(page: &str) -> &str {
            let end = page.find("-->").expect("host page opens with a comment");
            &page[end + "-->".len()..]
        }

        let dest = unique_temp_dir("web-drift-html");
        let ctx = test_context();

        generate_with_platforms(&dest, &ctx, None, false, None, ScaffoldPlatform::ALL).unwrap();

        let embedder =
            fs::read_to_string(repo_root().join("crates/frust-shell-web/platform/web/index.html"))
                .expect("reading crates/frust-shell-web/platform/web/index.html");
        let expected = body(&embedder)
            .replace(
                "<title>frust app</title>",
                &format!("<title>{}</title>", ctx.title_case_name),
            )
            .replace("./pkg/app.js", &format!("./pkg/{}.js", ctx.project_name));
        let scaffolded = fs::read_to_string(dest.join("web/index.html")).unwrap();
        assert_eq!(
            body(&scaffolded),
            expected.as_str(),
            "crates/frust-drive/templates/app/web.tmpl/index.html.tmpl has drifted from \
             crates/frust-shell-web/platform/web/index.html beyond its title and `?module=` default"
        );

        let _ = fs::remove_dir_all(&dest);
    }

    /// Every embedded plist-family template must be free of `--` inside XML
    /// comments. XML forbids the sequence in a comment, and while `plutil
    /// -lint` tolerates it, the strict parser `codesign` runs over an
    /// entitlements file (AMFIUnserializeXML) rejects the whole file — a
    /// scaffold shipping one fails every signed `frust build macos`
    /// (macbook-gate-r2 finding F-5, where the comments spelled codesign
    /// flags with their leading dashes).
    #[test]
    fn embedded_plist_templates_carry_no_double_hyphen_inside_comments() {
        fn walk(dir: &include_dir::Dir<'_>, offenders: &mut Vec<String>) {
            for entry in dir.entries() {
                match entry {
                    include_dir::DirEntry::Dir(sub) => walk(sub, offenders),
                    include_dir::DirEntry::File(file) => {
                        let name = file.path().to_string_lossy();
                        let plist_family = [
                            ".plist",
                            ".plist.tmpl",
                            ".entitlements",
                            ".entitlements.tmpl",
                        ]
                        .iter()
                        .any(|suffix| name.ends_with(suffix));
                        if !plist_family {
                            continue;
                        }
                        let Some(text) = file.contents_utf8() else {
                            continue;
                        };
                        let mut rest = text;
                        while let Some(open) = rest.find("<!--") {
                            let body = &rest[open + 4..];
                            let Some(close) = body.find("-->") else { break };
                            if body[..close].contains("--") {
                                offenders.push(name.to_string());
                            }
                            rest = &body[close + 3..];
                        }
                    }
                }
            }
        }

        let mut offenders = Vec::new();
        walk(&EMBEDDED_APP_TEMPLATE, &mut offenders);
        assert!(
            offenders.is_empty(),
            "plist-family templates with `--` inside an XML comment (breaks \
             codesign's AMFI parser): {offenders:?}"
        );
    }

    /// The design-system template root: file set, placeholder substitution,
    /// and the catalogs-off contract — the counterpart of
    /// `generate_produces_manifest_listed_files_with_substitutions` above.
    #[test]
    fn generate_design_system_produces_manifest_listed_files_with_substitutions() {
        let dest = unique_temp_dir("design-system-manifest-set");
        let ctx = test_design_system_context();

        let written = generate_design_system(&dest, &ctx, None, false).unwrap();
        assert_eq!(
            written,
            vec![
                PathBuf::from("Cargo.toml"),
                PathBuf::from("README.md"),
                PathBuf::from(".gitignore"),
                PathBuf::from("src/lib.rs"),
                PathBuf::from("src/tokens.rs"),
            ]
        );

        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(
            cargo_toml.contains("name = \"acme_design\""),
            "{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("frust = { package = \"frust-ui\", path = \"/path/to/frust\""),
            "{cargo_toml}"
        );
        // Catalogs-off contract: no built-in design-system feature, ever —
        // `frust` itself carries no design-system catalog (`default = []`),
        // so this template names no features on its `frust` dependency at
        // all.
        assert!(!cargo_toml.contains("default-features"), "{cargo_toml}");
        assert!(!cargo_toml.contains("\"glyph\""), "{cargo_toml}");
        assert!(!cargo_toml.contains("\"material\""), "{cargo_toml}");
        assert!(!cargo_toml.contains("\"cupertino\""), "{cargo_toml}");
        assert!(!cargo_toml.contains("{{"), "{cargo_toml}");

        let lib_rs = fs::read_to_string(dest.join("src/lib.rs")).unwrap();
        assert!(
            lib_rs.contains("frust::app!(App, setup = { acme_design::install(); });"),
            "{lib_rs}"
        );
        assert!(lib_rs.contains("pub fn install()"), "{lib_rs}");
        assert!(lib_rs.contains("pub struct BadgeView"), "{lib_rs}");
        assert!(lib_rs.contains("frust::authoring::"), "{lib_rs}");
        assert!(!lib_rs.contains("{{"), "{lib_rs}");

        let tokens_rs = fs::read_to_string(dest.join("src/tokens.rs")).unwrap();
        assert!(
            tokens_rs.contains("pub const DESIGN_LANGUAGE: &str = \"acme_design\";"),
            "{tokens_rs}"
        );
        assert!(
            tokens_rs.contains("DesignLanguage::Custom(DESIGN_LANGUAGE)"),
            "{tokens_rs}"
        );
        assert!(!tokens_rs.contains("{{"), "{tokens_rs}");

        let readme = fs::read_to_string(dest.join("README.md")).unwrap();
        assert!(readme.contains("# Acme Design"), "{readme}");
        assert!(readme.contains("acme_design::install();"), "{readme}");
        assert!(readme.contains("install-timing contract"), "{readme}");
        assert!(readme.contains("sibling plugin crates"), "{readme}");
        assert!(!readme.contains("{{"), "{readme}");

        let gitignore = fs::read_to_string(dest.join(".gitignore")).unwrap();
        assert!(gitignore.contains("/target"), "{gitignore}");

        // No app-only artifacts (no platform project, no manifest file).
        assert!(!dest.join("android").exists());
        assert!(!dest.join("ios").exists());
        assert!(!dest.join("frust.toml").exists());
        assert!(!dest.join("template_manifest.json").exists());
        assert!(!dest.join("Cargo.toml.tmpl").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_design_system_refuses_non_empty_dest_without_overwrite() {
        let dest = unique_temp_dir("design-system-refuse-non-empty");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_design_system_context();
        let err = generate_design_system(&dest, &ctx, None, false).unwrap_err();
        assert!(err.to_string().contains("existing.txt"), "{err}");

        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn generate_design_system_allows_non_empty_dest_with_overwrite() {
        let dest = unique_temp_dir("design-system-allow-overwrite");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("existing.txt"), b"x").unwrap();

        let ctx = test_design_system_context();
        assert!(generate_design_system(&dest, &ctx, None, true).is_ok());
        assert!(dest.join("existing.txt").exists());

        let _ = fs::remove_dir_all(&dest);
    }

    /// Two crate names render two independently-tagged themes: the identity
    /// tag isn't accidentally hardcoded to the fixture value used elsewhere
    /// in this file.
    #[test]
    fn generate_design_system_uses_the_given_crate_name_as_the_design_language_tag() {
        let dest = unique_temp_dir("design-system-other-name");
        let mut ctx = test_design_system_context();
        ctx.name = "widgetry".into();

        generate_design_system(&dest, &ctx, None, false).unwrap();

        let tokens_rs = fs::read_to_string(dest.join("src/tokens.rs")).unwrap();
        assert!(
            tokens_rs.contains("pub const DESIGN_LANGUAGE: &str = \"widgetry\";"),
            "{tokens_rs}"
        );
        let cargo_toml = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("name = \"widgetry\""), "{cargo_toml}");

        let _ = fs::remove_dir_all(&dest);
    }
}
