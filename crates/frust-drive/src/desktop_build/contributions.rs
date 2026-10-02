//! Plugin-declared desktop contributions, merged into the assembled bundle.
//!
//! `frust plugin add` never writes a desktop contribution into a project file
//! (see [`crate::plugin::Contribution::MacosPlistEntry`]'s doc comment): the
//! three desktop variants are only *recorded* by the registry and applied
//! here, fresh, on every `frust build macos|windows|linux`. Remove the plugin
//! dependency and the next build simply stops applying them — no project file
//! is left carrying a contribution its plugin no longer owns.
//!
//! **Everything written here lands under `build/desktop/`.** The merge targets
//! are the assembled bundle's own `Contents/Info.plist` and
//! `<identifier>.desktop` — copies, whichever way they were produced — plus,
//! for entitlements (which `codesign` reads from a file rather than from the
//! bundle), a generated `build/desktop/macos/<binary>.entitlements` beside the
//! `.app`. The project's own `macos/Info.plist`, `macos/app.entitlements` and
//! `linux/app.desktop` are user-owned and are only ever *read*.
//!
//! **An existing key always wins.** A key already present in the file is left
//! exactly as it is and reported as a [`BundleNote::PluginEntryPresent`] — a
//! hand-tuned usage description or category is the user's, and a plugin's
//! default has no business overwriting it.
//!
//! **A contribution that cannot be applied is a hard failure**
//! ([`DesktopBuildError::ContributionUnappliable`]), unlike the icon and
//! identity findings this pipeline degrades into notes. The reason is the
//! failure mode: a bundle that silently ships without a declared usage
//! description or entitlement is killed by the OS at *runtime*, in front of a
//! user, rather than at build time in front of the developer who can fix it.
//! The one carve-out is an entitlement on an **unsigned** build: entitlements
//! take effect only through a signature, so there is nothing to fail about —
//! that is one [`BundleNote::EntitlementsSkippedUnsigned`].

use std::fs;
use std::path::{Path, PathBuf};

use crate::plugin::{self, Contribution, DesktopContribution, PluginAddError};

use super::bundle::write_file;
use super::config::DesktopConfig;
use super::linux::{self, DESKTOP_ENTRY_GROUP, entry_value};
use super::macos::{self, xml_escape};
use super::{BundleNote, BundleReport, DesktopBuildError, DesktopBundleTarget};

/// The plist anchor a contributed key is inserted before: the **last**
/// `</dict>` in the file. An `Info.plist` may nest dictionaries
/// (`NSAppTransportSecurity`, `UTExportedTypeDeclarations`), so the *first*
/// close would land the key inside a nested dict where nothing reads it;
/// the last one is the root dict's own close.
const DICT_CLOSE: &str = "</dict>";

/// The entitlements file generated when a plugin contributes one: an empty
/// property list the contributed `<key>/<true/>` pairs are then merged into,
/// so a generated file and a merged project file go through the exact same
/// insertion path and come out formatted identically.
const ENTITLEMENTS_SKELETON: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
     \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
     <plist version=\"1.0\">\n<dict>\n</dict>\n</plist>\n";

/// The project's own entitlements file — read (never written) when a plugin
/// contributes an entitlement, and passed to `codesign` unchanged when none
/// does.
const PROJECT_ENTITLEMENTS_REL: &str = "macos/app.entitlements";

/// Human names for the three merge targets, for a note's own message.
const IN_INFO_PLIST: &str = "the bundle's `Info.plist`";
const IN_ENTITLEMENTS: &str = "the entitlements";
const IN_DESKTOP_ENTRY: &str = "the bundle's desktop entry";

/// The desktop contributions the plugins installed in the project at
/// `project_dir` owe a `target` bundle, in registry order.
///
/// A typed failure reading the project's `Cargo.toml` becomes
/// [`DesktopBuildError::PluginContributions`] rather than being swallowed —
/// with one exception: a directory with no `Cargo.toml` at all has no
/// dependency list to read installed plugins from, so the honest answer is
/// "no plugins" rather than a failure. Whether a missing manifest is a real
/// problem is the compile's verdict to give, and by the time this runs the
/// compile has already given it.
pub(super) fn collect(
    project_dir: &Path,
    target: DesktopBundleTarget,
) -> Result<Vec<DesktopContribution>, DesktopBuildError> {
    let rows = match plugin::desktop_contributions(project_dir) {
        Ok(rows) => rows,
        Err(PluginAddError::MissingProjectFile(_)) => Vec::new(),
        Err(err) => {
            return Err(DesktopBuildError::PluginContributions {
                reason: err.to_string(),
            });
        }
    };
    Ok(rows
        .into_iter()
        .filter(|row| applies_to(row.contribution, target))
        .collect())
}

/// Whether `contribution` is applied by a `target` bundle assembly.
///
/// Matched exhaustively rather than with a wildcard: a new
/// [`Contribution`] variant must be classified here deliberately, not
/// silently dropped (or silently applied) by a `_` arm.
fn applies_to(contribution: &Contribution, target: DesktopBundleTarget) -> bool {
    match contribution {
        Contribution::MacosPlistEntry { .. } | Contribution::MacosEntitlement { .. } => {
            target == DesktopBundleTarget::Macos
        }
        Contribution::LinuxDesktopEntry { .. } => target == DesktopBundleTarget::Linux,
        // The mobile/shared lane: applied to project files by `add_plugin`
        // itself, never by a desktop bundle assembly. Windows has no desktop
        // variant of its own in v1, so a Windows build applies nothing.
        Contribution::CargoDep { .. }
        | Contribution::ManifestPermission { .. }
        | Contribution::PlistEntry { .. }
        | Contribution::GradleModule { .. }
        | Contribution::SwiftPackageRef { .. }
        | Contribution::IosFramework { .. }
        | Contribution::AppCrateMacro { .. }
        | Contribution::CargoFeature { .. }
        | Contribution::ScaffoldFile { .. } => false,
    }
}

/// Merges `rows` into the assembled bundle `report` describes, and answers the
/// entitlements file `codesign` should be given (macOS only; `None` on every
/// other target, and on an unsigned or contribution-free build with no project
/// entitlements of its own).
///
/// Runs **after** the per-OS assembly and **before** `codesign`: a signature
/// covers the bundle's files as they are at signing time, so a plist merged
/// afterwards would invalidate it.
///
/// `report` is taken by `&mut` for one reason: a generated entitlements file
/// is a file this run wrote, so it joins [`BundleReport::artifacts`] like every
/// other written file.
pub(super) fn apply(
    project_dir: &Path,
    config: &DesktopConfig,
    report: &mut BundleReport,
    rows: &[DesktopContribution],
    notes: &mut Vec<BundleNote>,
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<PathBuf>, DesktopBuildError> {
    // Counted against the target rather than taken from `rows.len()`, so the
    // line is silent on a target with nothing to apply — a Windows build in
    // particular, which has no desktop variant of its own in v1.
    let applicable = rows
        .iter()
        .filter(|row| applies_to(row.contribution, report.target))
        .count();
    if applicable > 0 {
        on_line(&format!(
            "plugins: applying {applicable} desktop contribution(s)"
        ));
    }
    match report.target {
        DesktopBundleTarget::Macos => {
            merge_info_plist(&macos::info_plist_path(&report.root), rows, notes)?;
            resolve_entitlements(project_dir, config, rows, &mut report.artifacts, notes)
        }
        DesktopBundleTarget::Linux => {
            merge_desktop_entry(&linux::entry_path(&report.root, config), rows, notes)?;
            Ok(None)
        }
        // No Windows-lane variant exists in v1, so `collect` has already
        // filtered every row out: nothing to apply, and nothing to note.
        DesktopBundleTarget::Windows => Ok(None),
    }
}

/// Merges every [`Contribution::MacosPlistEntry`] in `rows` into the assembled
/// bundle's `Info.plist` — the build copy, so a generated and a copied plist
/// are handled identically and the project's own file is never touched.
fn merge_info_plist(
    plist: &Path,
    rows: &[DesktopContribution],
    notes: &mut Vec<BundleNote>,
) -> Result<(), DesktopBuildError> {
    let entries: Vec<&DesktopContribution> = rows
        .iter()
        .filter(|row| matches!(row.contribution, Contribution::MacosPlistEntry { .. }))
        .collect();
    let Some(first) = entries.first() else {
        return Ok(());
    };

    let mut text = read_text(plist, first)?;
    let mut changed = false;
    for row in entries {
        let Contribution::MacosPlistEntry {
            key,
            value,
            comment,
        } = row.contribution
        else {
            continue;
        };
        if has_root_key(&text, &xml_escape(key)) {
            notes.push(present_note(row, key, IN_INFO_PLIST));
            continue;
        }
        let block = format!(
            "{}\t<key>{}</key>\n\t<string>{}</string>\n",
            comment_line(comment, row.plugin_id),
            xml_escape(key),
            xml_escape(value)
        );
        text = insert_before_last(&text, DICT_CLOSE, &block)
            .ok_or_else(|| unappliable(row, no_anchor_reason(DICT_CLOSE, plist)))?;
        changed = true;
        notes.push(applied_note(row));
    }
    if changed {
        write_file(plist, &text)?;
    }
    Ok(())
}

/// The entitlements file `codesign` gets, applying every
/// [`Contribution::MacosEntitlement`] in `rows`:
///
/// - no entitlement contributions → the project's own `macos/app.entitlements`
///   iff it exists, exactly the behavior this pipeline had before plugins
///   could contribute one;
/// - contributions on an **unsigned** build → nothing at all, plus one
///   [`BundleNote::EntitlementsSkippedUnsigned`]: an entitlement without a
///   signature is not a degraded bundle, it is a no-op;
/// - contributions on a signing build → a **generated** file at
///   `build/desktop/macos/<binary>.entitlements`, either the project's own file with
///   the absent keys merged in or a minimal plist of just the contributed
///   keys.
///
/// The generated file is removed whenever this run does not write one, so a
/// later packaging pass (or a later build) can never pick up a stale set of
/// entitlements from a plugin the project no longer has.
fn resolve_entitlements(
    project_dir: &Path,
    config: &DesktopConfig,
    rows: &[DesktopContribution],
    artifacts: &mut Vec<PathBuf>,
    notes: &mut Vec<BundleNote>,
) -> Result<Option<PathBuf>, DesktopBuildError> {
    let project_file = project_dir.join(PROJECT_ENTITLEMENTS_REL);
    let project_file = project_file.is_file().then_some(project_file);
    let generated = generated_entitlements_path(project_dir, config);

    let entitlements: Vec<&DesktopContribution> = rows
        .iter()
        .filter(|row| matches!(row.contribution, Contribution::MacosEntitlement { .. }))
        .collect();
    if entitlements.is_empty() {
        remove_generated(&generated)?;
        return Ok(project_file);
    }
    if config.macos_signing_identity.is_none() {
        remove_generated(&generated)?;
        notes.push(BundleNote::EntitlementsSkippedUnsigned {
            count: entitlements.len(),
        });
        return Ok(None);
    }

    let mut text = match &project_file {
        Some(path) => read_text(path, entitlements[0])?,
        None => ENTITLEMENTS_SKELETON.to_string(),
    };
    for row in entitlements {
        let Contribution::MacosEntitlement { key, comment } = row.contribution else {
            continue;
        };
        if has_root_key(&text, &xml_escape(key)) {
            notes.push(present_note(row, key, IN_ENTITLEMENTS));
            continue;
        }
        let block = format!(
            "{}\t<key>{}</key>\n\t<true/>\n",
            comment_line(comment, row.plugin_id),
            xml_escape(key)
        );
        let anchor_missing = || {
            let source = project_file.as_deref().unwrap_or(&generated);
            unappliable(row, no_anchor_reason(DICT_CLOSE, source))
        };
        text = insert_before_last(&text, DICT_CLOSE, &block).ok_or_else(anchor_missing)?;
        notes.push(applied_note(row));
    }
    write_file(&generated, &text)?;
    artifacts.push(generated.clone());
    Ok(Some(generated))
}

/// Merges every [`Contribution::LinuxDesktopEntry`] in `rows` into the
/// assembled bundle's `<identifier>.desktop` — again the build copy, generated
/// or copied alike.
///
/// A key already in the `[Desktop Entry]` group wins; a new one is appended at
/// the end of that group (before the next `[group]` header, if any), which is
/// where the format says a key belongs.
fn merge_desktop_entry(
    entry: &Path,
    rows: &[DesktopContribution],
    notes: &mut Vec<BundleNote>,
) -> Result<(), DesktopBuildError> {
    let entries: Vec<&DesktopContribution> = rows
        .iter()
        .filter(|row| matches!(row.contribution, Contribution::LinuxDesktopEntry { .. }))
        .collect();
    let Some(first) = entries.first() else {
        return Ok(());
    };

    let mut text = read_text(entry, first)?;
    let mut changed = false;
    for row in entries {
        let Contribution::LinuxDesktopEntry {
            key,
            value,
            comment,
        } = row.contribution
        else {
            continue;
        };
        if entry_value(&text, key).is_some() {
            notes.push(present_note(row, key, IN_DESKTOP_ENTRY));
            continue;
        }
        // Every part goes through the entry's own line sanitizer: a value
        // carrying a newline would otherwise append a second key of its own.
        let block = format!(
            "# {} (frust plugin: {})\n{}={}\n",
            linux::sanitize(comment),
            row.plugin_id,
            linux::sanitize(key),
            linux::sanitize(value)
        );
        text = append_to_entry_group(&text, &block).ok_or_else(|| {
            unappliable(
                row,
                format!(
                    "the assembled desktop entry at '{}' has no `[{DESKTOP_ENTRY_GROUP}]` group \
                     to append a key to",
                    entry.display()
                ),
            )
        })?;
        changed = true;
        notes.push(applied_note(row));
    }
    if changed {
        write_file(entry, &text)?;
    }
    Ok(())
}

/// The entitlements file a *packaging* pass should sign with when the build
/// itself has no answer to give: the merged file a `frust build macos`
/// generated beside the `.app` when a plugin contributed an entitlement, else
/// the project's own `macos/app.entitlements`, else nothing.
///
/// **A fallback, not the primary route.** A packaging pass takes
/// [`BundleReport::entitlements`] — the exact path *this* build's `codesign`
/// used — and reaches for this probe only when that is `None`: the
/// unsigned-build-handed-to-a-packager-identity case, where the assembly
/// resolved no entitlements at all (an entitlement without a signature is a
/// no-op) but `cargo-packager`'s own codesign pass over the `.app` it
/// synthesizes still needs one. Probing is second-best precisely because it
/// trusts whatever file sits at the predictable output path.
pub(super) fn entitlements_for_packaging(
    project_dir: &Path,
    config: &DesktopConfig,
) -> Option<PathBuf> {
    let generated = generated_entitlements_path(project_dir, config);
    if generated.is_file() {
        return Some(generated);
    }
    let project_file = project_dir.join(PROJECT_ENTITLEMENTS_REL);
    project_file.is_file().then_some(project_file)
}

/// `build/desktop/macos/<binary>.entitlements` — a generated artifact beside
/// the `.app`, never a file in the project.
fn generated_entitlements_path(project_dir: &Path, config: &DesktopConfig) -> PathBuf {
    DesktopBundleTarget::Macos
        .output_dir(project_dir)
        .join(format!("{}.entitlements", config.binary_name))
}

/// Deletes a previously generated entitlements file, if one is there.
fn remove_generated(path: &Path) -> Result<(), DesktopBuildError> {
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(path).map_err(|source| DesktopBuildError::Io {
        action: "removing the previously generated entitlements at",
        path: path.to_path_buf(),
        source,
    })
}

/// The XML comment written above a contributed plist/entitlements key, naming
/// the plugin that owns it — so a reader of the built bundle can tell a
/// plugin's key from the project's own.
///
/// `--` cannot appear inside an XML comment, and a plist that does not parse
/// is a bundle that does not launch. Registry entries are static, trusted
/// strings, but the sanitization stays defensive rather than assuming that
/// forever (the same tone [`crate::plugin::PluginAddError::UnsafeScaffoldPath`]
/// takes) — and it covers the **whole** composed text, plugin id included,
/// since a plugin id is just as much a string as the comment is.
fn comment_line(comment: &str, plugin_id: &str) -> String {
    let text = xml_comment_text(&format!("{comment} (frust plugin: {plugin_id})"));
    format!("\t<!-- {text} -->\n")
}

/// `raw` reduced to text that can legally sit inside `<!-- … -->`.
///
/// Every run of `-` collapses to a single `-`, which is a fixed point by
/// construction — a `replace("--", "-")` pass is not (`---` leaves a surviving
/// `--`, and a malformed comment is an unparseable plist, i.e. a dead `.app`).
/// A trailing `-` goes too: it would sit against the closing delimiter.
fn xml_comment_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch == '-' && prev_dash {
            continue;
        }
        prev_dash = ch == '-';
        out.push(ch);
    }
    while out.ends_with('-') || out.ends_with(' ') {
        out.pop();
    }
    out
}

fn applied_note(row: &DesktopContribution) -> BundleNote {
    BundleNote::PluginContribution {
        plugin_id: row.plugin_id,
        description: row.contribution.describe(),
    }
}

fn present_note(row: &DesktopContribution, key: &str, file: &'static str) -> BundleNote {
    BundleNote::PluginEntryPresent {
        plugin_id: row.plugin_id,
        key: key.to_string(),
        file,
    }
}

fn unappliable(row: &DesktopContribution, reason: String) -> DesktopBuildError {
    DesktopBuildError::ContributionUnappliable {
        plugin_id: row.plugin_id,
        description: row.contribution.describe(),
        reason,
    }
}

fn no_anchor_reason(anchor: &str, path: &Path) -> String {
    format!("'{}' has no `{anchor}` to insert before", path.display())
}

/// Reads a file the merge is about to edit, blaming `row` when it cannot be
/// read as text — a binary (rather than XML) `Info.plist` is the realistic
/// case, and a contribution that cannot be applied is never silently skipped.
fn read_text(path: &Path, row: &DesktopContribution) -> Result<String, DesktopBuildError> {
    fs::read_to_string(path).map_err(|err| {
        unappliable(
            row,
            format!(
                "'{}' could not be read as UTF-8 text ({err})",
                path.display()
            ),
        )
    })
}

/// Whether `escaped_key` is already a key of the plist's **root** dictionary —
/// the one question "is this contributed key present?" actually asks, for both
/// merge targets.
///
/// A whole-file substring search answers a different (and wrong) question in
/// two ways, each of which silently drops a contribution the OS then kills the
/// app over at runtime:
///
/// - a root-level key whose *name* also appears inside a nested dictionary
///   (`NSAppTransportSecurity`'s sub-keys, `CFBundleURLTypes`,
///   `UTExportedTypeDeclarations`) or inside an XML comment would be read as
///   present and never inserted at root, where it is actually read;
/// - the insertion writes [`xml_escape`]d text, so a key needing escaping
///   could never match its own prior insertion — a second merge over the
///   merged output would insert it again.
///
/// So: comments are stripped first, then `<dict>`/`</dict>` are depth-counted
/// and a `<key>` counts only at depth 1 (inside the plist's outer dict), and
/// the comparison is against the **escaped** spelling that insertion writes.
/// Tags are matched in their canonical, no-inner-whitespace spelling — the
/// same assumption the [`DICT_CLOSE`] insertion anchor already makes, and the
/// spelling every plist this pipeline reads (Apple's tools', this module's
/// own) is written in.
fn has_root_key(text: &str, escaped_key: &str) -> bool {
    let text = strip_xml_comments(text);
    let needle = format!("<key>{escaped_key}</key>");
    let mut depth = 0usize;
    let mut idx = 0usize;
    while let Some(found) = text[idx..].find('<') {
        let at = idx + found;
        let tail = &text[at..];
        if tail.starts_with("<dict>") {
            depth += 1;
            idx = at + "<dict>".len();
        } else if tail.starts_with(DICT_CLOSE) {
            depth = depth.saturating_sub(1);
            idx = at + DICT_CLOSE.len();
        } else if depth == 1 && tail.starts_with(&needle) {
            return true;
        } else {
            idx = at + 1;
        }
    }
    false
}

/// `text` with every `<!-- … -->` span removed, so a key named only inside a
/// comment is not mistaken for a declared one. An unterminated `<!--` comments
/// out the rest of the file, exactly as an XML parser would read it.
fn strip_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("-->") else {
            return out;
        };
        rest = &rest[start + end + "-->".len()..];
    }
    out.push_str(rest);
    out
}

/// Inserts `block` before the **last** occurrence of `anchor`, or `None` when
/// the text carries no such anchor (in which case nothing is written — the
/// caller turns that into a typed refusal).
fn insert_before_last(text: &str, anchor: &str, block: &str) -> Option<String> {
    let idx = text.rfind(anchor)?;
    let mut out = String::with_capacity(text.len() + block.len());
    out.push_str(&text[..idx]);
    out.push_str(block);
    out.push_str(&text[idx..]);
    Some(out)
}

/// Appends `block` at the end of a desktop entry's `[Desktop Entry]` group:
/// after the group's last **content** line as [`linux::is_content_line`]
/// defines it — the same definition [`entry_value`]'s lookup uses — so a
/// following `[Desktop Action …]` header, the blank line separating it, and
/// any comment block introducing it all stay where they are. `None` when the
/// file has no `[Desktop Entry]` group at all.
fn append_to_entry_group(text: &str, block: &str) -> Option<String> {
    let mut in_group = false;
    let mut seen_group = false;
    let mut insert_at = 0usize;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        offset += line.len();
        if let Some(rest) = trimmed.strip_prefix('[') {
            in_group = rest.trim_end_matches(']').trim() == DESKTOP_ENTRY_GROUP;
            if in_group {
                seen_group = true;
                insert_at = offset;
            }
            continue;
        }
        if in_group && linux::is_content_line(trimmed) {
            insert_at = offset;
        }
    }
    if !seen_group {
        return None;
    }
    let mut out = String::with_capacity(text.len() + block.len() + 1);
    out.push_str(&text[..insert_at]);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    out.push_str(&text[insert_at..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Synthetic contributions: the real registry carries zero desktop rows
    // (this feature is the seam, not a backfill), and `DesktopContribution`
    // borrows `&'static Contribution`, so the fixtures are `static` items.
    static CAMERA_USAGE: Contribution = Contribution::MacosPlistEntry {
        key: "NSCameraUsageDescription",
        value: "Scan a document & <sign> it",
        comment: "Camera access",
    };
    static MIC_USAGE: Contribution = Contribution::MacosPlistEntry {
        key: "NSMicrophoneUsageDescription",
        value: "Record a voice note",
        comment: "Microphone access",
    };
    static NETWORK_ENTITLEMENT: Contribution = Contribution::MacosEntitlement {
        key: "com.apple.security.network.client",
        comment: "Outbound network access",
    };
    static CAMERA_ENTITLEMENT: Contribution = Contribution::MacosEntitlement {
        key: "com.apple.security.device.camera",
        comment: "Camera device access",
    };
    static MIME_TYPE: Contribution = Contribution::LinuxDesktopEntry {
        key: "MimeType",
        value: "image/png;",
        comment: "Handled file types",
    };
    static CATEGORIES: Contribution = Contribution::LinuxDesktopEntry {
        key: "Categories",
        value: "Graphics;",
        comment: "Launcher categories",
    };
    static CARGO_DEP: Contribution = Contribution::CargoDep {
        name: "frust-camera",
    };
    static COMMENT_WITH_DASHES: Contribution = Contribution::MacosPlistEntry {
        key: "NSFooUsageDescription",
        value: "ok",
        comment: "a -- b --> c",
    };
    static MULTILINE_ENTRY: Contribution = Contribution::LinuxDesktopEntry {
        key: "MimeType",
        value: "image/png;\nExec=/bin/sh",
        comment: "Handled\ntypes",
    };
    /// A key whose own spelling needs XML escaping: the file can only ever
    /// carry it escaped, so the presence check has to compare escaped too.
    static ESCAPED_KEY: Contribution = Contribution::MacosPlistEntry {
        key: "NSFoo&Bar",
        value: "ok",
        comment: "Escaped key",
    };

    fn row(contribution: &'static Contribution) -> DesktopContribution {
        DesktopContribution {
            plugin_id: "camera",
            contribution,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-desktop-contrib-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(toml: &str) -> DesktopConfig {
        DesktopConfig::resolve(
            Path::new("/projects/my_app"),
            &manifest::parse(toml).unwrap(),
        )
        .unwrap()
    }

    fn signing_config() -> DesktopConfig {
        config(
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n\n\
             [macos]\nsigning-identity = \"Developer ID Application: Example\"\n",
        )
    }

    fn unsigned_config() -> DesktopConfig {
        config("[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n")
    }

    /// The per-target filter is a filter, not a pass-through: a macOS build
    /// takes the two macOS variants, a Linux build the Linux one, and a
    /// Windows build — which has no variant of its own in v1 — takes nothing.
    #[test]
    fn each_target_takes_only_its_own_variants() {
        let cases = [
            (DesktopBundleTarget::Macos, [true, true, false, false]),
            (DesktopBundleTarget::Linux, [false, false, true, false]),
            (DesktopBundleTarget::Windows, [false, false, false, false]),
        ];
        for (target, expected) in cases {
            let actual = [
                applies_to(&CAMERA_USAGE, target),
                applies_to(&NETWORK_ENTITLEMENT, target),
                applies_to(&MIME_TYPE, target),
                applies_to(&CARGO_DEP, target),
            ];
            assert_eq!(actual, expected, "{target}");
        }
    }

    /// The real registry carries no desktop contributions today, so a real
    /// project collects an empty set on every target — the no-op contract
    /// every existing build keeps.
    #[test]
    fn a_project_with_no_plugin_dependencies_collects_nothing() {
        let dir = temp_dir("collect-empty");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my_app\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        )
        .unwrap();
        for target in [
            DesktopBundleTarget::Macos,
            DesktopBundleTarget::Windows,
            DesktopBundleTarget::Linux,
        ] {
            assert!(collect(&dir, target).unwrap().is_empty(), "{target}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// A directory with no `Cargo.toml` has no dependency list to read, so
    /// there are no installed plugins to collect — never a build failure (the
    /// compile already had its say about the missing manifest).
    #[test]
    fn a_directory_without_a_cargo_toml_collects_nothing() {
        let dir = temp_dir("collect-no-manifest");
        assert!(
            collect(&dir, DesktopBundleTarget::Macos)
                .unwrap()
                .is_empty()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// …but a `Cargo.toml` that cannot be parsed is reported rather than
    /// silently read as "no plugins".
    #[test]
    fn an_unparseable_cargo_toml_is_a_typed_failure() {
        let dir = temp_dir("collect-broken-manifest");
        fs::write(dir.join("Cargo.toml"), "[package\nname = ").unwrap();
        let err = collect(&dir, DesktopBundleTarget::Macos).unwrap_err();
        assert!(
            matches!(err, DesktopBuildError::PluginContributions { .. }),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A plist with a nested dictionary: the contributed key belongs to the
    /// **root** dict, so it is inserted before the last `</dict>` — the first
    /// one closes `NSAppTransportSecurity`, where nothing would read it.
    fn nested_plist() -> String {
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\
         \t<key>CFBundleExecutable</key>\n\t<string>my_app</string>\n\
         \t<key>NSAppTransportSecurity</key>\n\t<dict>\n\
         \t\t<key>NSAllowsArbitraryLoads</key>\n\t\t<true/>\n\t</dict>\n\
         </dict>\n</plist>\n"
            .to_string()
    }

    #[test]
    fn a_contributed_plist_key_is_inserted_before_the_last_dict_close() {
        let dir = temp_dir("plist-nested");
        let plist = dir.join("Info.plist");
        fs::write(&plist, nested_plist()).unwrap();

        let mut notes = Vec::new();
        merge_info_plist(&plist, &[row(&CAMERA_USAGE)], &mut notes).unwrap();

        let merged = fs::read_to_string(&plist).unwrap();
        // Inside the root dict, after the nested dictionary's own close.
        let key_at = merged.find("<key>NSCameraUsageDescription</key>").unwrap();
        assert!(
            key_at > merged.find("NSAllowsArbitraryLoads").unwrap(),
            "{merged}"
        );
        assert!(key_at < merged.rfind("</dict>").unwrap(), "{merged}");
        // The value is XML-escaped, and the comment names the plugin.
        assert!(
            merged.contains("<string>Scan a document &amp; &lt;sign&gt; it</string>"),
            "{merged}"
        );
        assert!(
            merged.contains("<!-- Camera access (frust plugin: camera) -->"),
            "{merged}"
        );
        assert!(
            matches!(notes.as_slice(), [BundleNote::PluginContribution { .. }]),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A key that appears only *inside a nested dictionary* is not a key of
    /// the root dict, so it is still inserted where the OS reads it. Read as
    /// present it would be dropped with nothing but a note, and a bundle
    /// missing a declared usage description is killed at runtime.
    #[test]
    fn a_key_present_only_in_a_nested_dict_is_still_inserted_at_the_root() {
        let dir = temp_dir("plist-nested-namesake");
        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            "<plist version=\"1.0\">\n<dict>\n\
             \t<key>NSAppTransportSecurity</key>\n\t<dict>\n\
             \t\t<key>NSCameraUsageDescription</key>\n\t\t<string>nested</string>\n\t</dict>\n\
             </dict>\n</plist>\n",
        )
        .unwrap();

        let mut notes = Vec::new();
        merge_info_plist(&plist, &[row(&CAMERA_USAGE)], &mut notes).unwrap();

        let merged = fs::read_to_string(&plist).unwrap();
        assert_eq!(
            merged
                .matches("<key>NSCameraUsageDescription</key>")
                .count(),
            2,
            "{merged}"
        );
        // The root-level one is the contributed value, after the nested close.
        let root_at = merged.rfind("<key>NSCameraUsageDescription</key>").unwrap();
        assert!(root_at > merged.find("\t</dict>").unwrap(), "{merged}");
        assert!(merged[root_at..].contains("Scan a document"), "{merged}");
        assert!(merged.contains("<string>nested</string>"), "{merged}");
        assert!(
            matches!(notes.as_slice(), [BundleNote::PluginContribution { .. }]),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A key named only inside an XML comment is not declared at all — a
    /// commented-out key is exactly the case a plugin's default should fill.
    #[test]
    fn a_key_named_only_in_a_comment_is_still_inserted() {
        let dir = temp_dir("plist-commented-key");
        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            "<plist version=\"1.0\">\n<dict>\n\
             \t<!-- <key>NSCameraUsageDescription</key> was removed for now -->\n\
             </dict>\n</plist>\n",
        )
        .unwrap();

        let mut notes = Vec::new();
        merge_info_plist(&plist, &[row(&CAMERA_USAGE)], &mut notes).unwrap();

        let merged = fs::read_to_string(&plist).unwrap();
        assert!(merged.contains("Scan a document"), "{merged}");
        assert!(
            matches!(notes.as_slice(), [BundleNote::PluginContribution { .. }]),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A key whose spelling needs escaping is written escaped, so the presence
    /// check must compare escaped too — otherwise a second merge over the
    /// merged output inserts it all over again. Merging twice is idempotent.
    #[test]
    fn a_key_needing_xml_escaping_is_inserted_once_and_then_reported_present() {
        let dir = temp_dir("plist-escaped-key");
        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            "<plist version=\"1.0\">\n<dict>\n</dict>\n</plist>\n",
        )
        .unwrap();

        let mut notes = Vec::new();
        merge_info_plist(&plist, &[row(&ESCAPED_KEY)], &mut notes).unwrap();
        let once = fs::read_to_string(&plist).unwrap();
        assert!(once.contains("<key>NSFoo&amp;Bar</key>"), "{once}");

        merge_info_plist(&plist, &[row(&ESCAPED_KEY)], &mut notes).unwrap();
        let twice = fs::read_to_string(&plist).unwrap();
        assert_eq!(twice, once, "a second merge rewrote the plist");
        assert!(
            matches!(
                notes.as_slice(),
                [
                    BundleNote::PluginContribution { .. },
                    BundleNote::PluginEntryPresent { key, .. }
                ] if key == "NSFoo&Bar"
            ),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// The entitlements half of the same check: an entitlement nested inside
    /// another key's dictionary is not a root entitlement, so the contributed
    /// one is still merged in.
    #[test]
    fn an_entitlement_present_only_in_a_nested_dict_is_still_merged_at_the_root() {
        let dir = temp_dir("entitlements-nested-namesake");
        let project_file = dir.join(PROJECT_ENTITLEMENTS_REL);
        fs::create_dir_all(project_file.parent().unwrap()).unwrap();
        fs::write(
            &project_file,
            "<plist version=\"1.0\">\n<dict>\n\
             \t<key>com.apple.security.application-groups</key>\n\t<dict>\n\
             \t\t<key>com.apple.security.device.camera</key>\n\t\t<true/>\n\t</dict>\n\
             </dict>\n</plist>\n",
        )
        .unwrap();

        let config = signing_config();
        let mut notes = Vec::new();
        let resolved = resolve_entitlements(
            &dir,
            &config,
            &[row(&CAMERA_ENTITLEMENT)],
            &mut Vec::new(),
            &mut notes,
        )
        .unwrap()
        .expect("a generated entitlements path");

        let merged = fs::read_to_string(&resolved).unwrap();
        assert_eq!(
            merged
                .matches("<key>com.apple.security.device.camera</key>")
                .count(),
            2,
            "{merged}"
        );
        assert!(
            matches!(notes.as_slice(), [BundleNote::PluginContribution { .. }]),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A hand-edited value is never overwritten: the key already there stays,
    /// and only the absent one is inserted.
    #[test]
    fn an_existing_plist_key_wins_and_is_reported() {
        let dir = temp_dir("plist-present");
        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            "<plist version=\"1.0\"><dict>\n\
             \t<key>NSCameraUsageDescription</key>\n\t<string>My own words</string>\n\
             </dict></plist>\n",
        )
        .unwrap();

        let mut notes = Vec::new();
        merge_info_plist(&plist, &[row(&CAMERA_USAGE), row(&MIC_USAGE)], &mut notes).unwrap();

        let merged = fs::read_to_string(&plist).unwrap();
        assert!(merged.contains("<string>My own words</string>"), "{merged}");
        assert!(!merged.contains("Scan a document"), "{merged}");
        assert!(
            merged.contains("<key>NSMicrophoneUsageDescription</key>"),
            "{merged}"
        );
        assert!(
            matches!(
                notes.as_slice(),
                [
                    BundleNote::PluginEntryPresent { key, .. },
                    BundleNote::PluginContribution { .. }
                ] if key == "NSCameraUsageDescription"
            ),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A plist with no `</dict>` at all cannot carry the key, and a bundle
    /// shipping without a declared usage description is killed at runtime —
    /// so this refuses instead of skipping.
    #[test]
    fn a_plist_without_a_dict_close_refuses_the_build() {
        let dir = temp_dir("plist-malformed");
        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            "<plist version=\"1.0\">not a property list</plist>\n",
        )
        .unwrap();

        let err = merge_info_plist(&plist, &[row(&CAMERA_USAGE)], &mut Vec::new()).unwrap_err();
        assert!(
            matches!(
                &err,
                DesktopBuildError::ContributionUnappliable { plugin_id, .. } if *plugin_id == "camera"
            ),
            "{err}"
        );
        assert!(err.to_string().contains("</dict>"), "{err}");
        // Nothing was rewritten.
        assert_eq!(
            fs::read_to_string(&plist).unwrap(),
            "<plist version=\"1.0\">not a property list</plist>\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// With no contributions at all the plist is not even read — proven by
    /// pointing the merge at a file that does not exist.
    #[test]
    fn no_contributions_touches_no_plist() {
        let mut notes = Vec::new();
        merge_info_plist(Path::new("/nonexistent/Info.plist"), &[], &mut notes).unwrap();
        merge_info_plist(
            Path::new("/nonexistent/Info.plist"),
            &[row(&NETWORK_ENTITLEMENT)],
            &mut notes,
        )
        .unwrap();
        assert!(notes.is_empty(), "{notes:?}");
    }

    /// The project's own entitlements file is read, merged, and written to a
    /// **generated** file beside the `.app` — the project file itself is
    /// byte-for-byte untouched.
    #[test]
    fn entitlements_merge_generates_a_build_file_and_never_edits_the_project_one() {
        let dir = temp_dir("entitlements-merge");
        let project_file = dir.join(PROJECT_ENTITLEMENTS_REL);
        fs::create_dir_all(project_file.parent().unwrap()).unwrap();
        let original = "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\
                        \t<key>com.apple.security.network.client</key>\n\t<true/>\n\
                        </dict>\n</plist>\n";
        fs::write(&project_file, original).unwrap();

        let config = signing_config();
        let mut notes = Vec::new();
        let mut artifacts = Vec::new();
        let resolved = resolve_entitlements(
            &dir,
            &config,
            &[row(&NETWORK_ENTITLEMENT), row(&CAMERA_ENTITLEMENT)],
            &mut artifacts,
            &mut notes,
        )
        .unwrap()
        .expect("a generated entitlements path");

        assert_eq!(
            resolved,
            dir.join("build/desktop/macos/my_app.entitlements")
        );
        // A file this run wrote, reported like every other written file.
        assert_eq!(artifacts, vec![resolved.clone()]);
        assert_eq!(fs::read_to_string(&project_file).unwrap(), original);
        let merged = fs::read_to_string(&resolved).unwrap();
        assert!(
            merged.contains("<key>com.apple.security.device.camera</key>\n\t<true/>"),
            "{merged}"
        );
        assert_eq!(
            merged.matches("com.apple.security.network.client").count(),
            1,
            "{merged}"
        );
        assert!(
            matches!(
                notes.as_slice(),
                [
                    BundleNote::PluginEntryPresent { .. },
                    BundleNote::PluginContribution { .. }
                ]
            ),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A project with no entitlements of its own gets a generated minimal
    /// plist carrying just the contributed keys.
    #[test]
    fn entitlements_are_generated_from_scratch_when_the_project_has_none() {
        let dir = temp_dir("entitlements-generated");
        let config = signing_config();
        let mut notes = Vec::new();
        let resolved = resolve_entitlements(
            &dir,
            &config,
            &[row(&CAMERA_ENTITLEMENT)],
            &mut Vec::new(),
            &mut notes,
        )
        .unwrap()
        .expect("a generated entitlements path");

        let merged = fs::read_to_string(&resolved).unwrap();
        assert!(merged.starts_with("<?xml version=\"1.0\""), "{merged}");
        assert!(
            merged.contains("<!-- Camera device access (frust plugin: camera) -->"),
            "{merged}"
        );
        assert!(
            merged.contains("<key>com.apple.security.device.camera</key>\n\t<true/>"),
            "{merged}"
        );
        assert!(merged.trim_end().ends_with("</plist>"), "{merged}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An entitlement without a signature does nothing at all, so an unsigned
    /// build applies none of them — one note, no file, no error.
    #[test]
    fn an_unsigned_build_skips_entitlements_with_a_note() {
        let dir = temp_dir("entitlements-unsigned");
        let config = unsigned_config();
        let mut notes = Vec::new();
        let mut artifacts = Vec::new();
        let resolved = resolve_entitlements(
            &dir,
            &config,
            &[row(&NETWORK_ENTITLEMENT), row(&CAMERA_ENTITLEMENT)],
            &mut artifacts,
            &mut notes,
        )
        .unwrap();

        assert_eq!(resolved, None);
        assert!(artifacts.is_empty(), "{artifacts:?}");
        assert!(!dir.join("build/desktop/macos/my_app.entitlements").exists());
        assert_eq!(
            notes,
            vec![BundleNote::EntitlementsSkippedUnsigned { count: 2 }]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// No entitlement contributions is the pre-plugin behavior, unchanged:
    /// the project's own file iff it exists, and no generated file left over
    /// from an earlier build that did contribute one.
    #[test]
    fn without_contributions_the_projects_own_entitlements_are_used_and_stale_ones_removed() {
        let dir = temp_dir("entitlements-none");
        let config = signing_config();
        let stale = dir.join("build/desktop/macos/my_app.entitlements");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "<plist/>").unwrap();

        let mut notes = Vec::new();
        assert_eq!(
            resolve_entitlements(&dir, &config, &[], &mut Vec::new(), &mut notes).unwrap(),
            None
        );
        assert!(
            !stale.exists(),
            "a stale generated entitlements file survived"
        );

        let project_file = dir.join(PROJECT_ENTITLEMENTS_REL);
        fs::create_dir_all(project_file.parent().unwrap()).unwrap();
        fs::write(&project_file, "<plist><dict/></plist>").unwrap();
        assert_eq!(
            resolve_entitlements(&dir, &config, &[], &mut Vec::new(), &mut notes).unwrap(),
            Some(project_file)
        );
        assert!(notes.is_empty(), "{notes:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A packaging pass prefers the generated file, then the project's own,
    /// then nothing.
    #[test]
    fn packaging_entitlements_prefer_the_generated_file() {
        let dir = temp_dir("entitlements-packaging");
        let config = signing_config();
        assert_eq!(entitlements_for_packaging(&dir, &config), None);

        let project_file = dir.join(PROJECT_ENTITLEMENTS_REL);
        fs::create_dir_all(project_file.parent().unwrap()).unwrap();
        fs::write(&project_file, "<plist/>").unwrap();
        assert_eq!(
            entitlements_for_packaging(&dir, &config),
            Some(project_file)
        );

        let generated = dir.join("build/desktop/macos/my_app.entitlements");
        fs::create_dir_all(generated.parent().unwrap()).unwrap();
        fs::write(&generated, "<plist/>").unwrap();
        assert_eq!(entitlements_for_packaging(&dir, &config), Some(generated));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The Linux half: a new key lands at the end of the `[Desktop Entry]`
    /// group — before a following `[Desktop Action …]` header, which owns its
    /// own keys — and an existing key wins.
    #[test]
    fn a_contributed_desktop_key_appends_to_the_entry_group() {
        let dir = temp_dir("desktop-append");
        let entry = dir.join("dev.f0x.my_app.desktop");
        fs::write(
            &entry,
            "[Desktop Entry]\nType=Application\nCategories=Utility;\n\n\
             [Desktop Action new]\nName=New Window\n",
        )
        .unwrap();

        let mut notes = Vec::new();
        merge_desktop_entry(&entry, &[row(&MIME_TYPE), row(&CATEGORIES)], &mut notes).unwrap();

        let merged = fs::read_to_string(&entry).unwrap();
        let mime_at = merged.find("MimeType=image/png;").unwrap();
        assert!(
            mime_at > merged.find("Type=Application").unwrap(),
            "{merged}"
        );
        assert!(
            mime_at < merged.find("[Desktop Action new]").unwrap(),
            "{merged}"
        );
        assert!(
            merged.contains("# Handled file types (frust plugin: camera)"),
            "{merged}"
        );
        // The user's own `Categories=` is untouched.
        assert!(merged.contains("Categories=Utility;"), "{merged}");
        assert!(!merged.contains("Categories=Graphics;"), "{merged}");
        assert!(
            matches!(
                notes.as_slice(),
                [
                    BundleNote::PluginContribution { .. },
                    BundleNote::PluginEntryPresent { key, .. }
                ] if key == "Categories"
            ),
            "{notes:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// An entry with no trailing newline, and no second group, still appends
    /// cleanly at the end.
    #[test]
    fn a_desktop_entry_without_a_trailing_newline_still_appends() {
        let dir = temp_dir("desktop-no-newline");
        let entry = dir.join("app.desktop");
        fs::write(&entry, "[Desktop Entry]\nType=Application").unwrap();
        merge_desktop_entry(&entry, &[row(&MIME_TYPE)], &mut Vec::new()).unwrap();
        let merged = fs::read_to_string(&entry).unwrap();
        assert!(merged.ends_with("MimeType=image/png;\n"), "{merged:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Registry entries are static, trusted strings — and are still filtered:
    /// a value carrying a newline cannot append a second desktop-entry key,
    /// and a comment carrying `--` cannot end the XML comment that wraps it
    /// (a plist that doesn't parse is a bundle that doesn't launch).
    #[test]
    fn a_contributed_value_cannot_inject_a_second_key_or_break_a_comment() {
        let dir = temp_dir("contrib-injection");
        let entry = dir.join("app.desktop");
        fs::write(&entry, "[Desktop Entry]\nType=Application\n").unwrap();
        merge_desktop_entry(&entry, &[row(&MULTILINE_ENTRY)], &mut Vec::new()).unwrap();
        let merged = fs::read_to_string(&entry).unwrap();
        assert_eq!(
            merged.lines().filter(|l| l.starts_with("Exec=")).count(),
            0,
            "{merged}"
        );
        assert!(
            merged.contains("MimeType=image/png;Exec=/bin/sh"),
            "{merged}"
        );

        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            "<plist version=\"1.0\">\n<dict>\n</dict>\n</plist>\n",
        )
        .unwrap();
        merge_info_plist(&plist, &[row(&COMMENT_WITH_DASHES)], &mut Vec::new()).unwrap();
        let merged = fs::read_to_string(&plist).unwrap();
        assert!(
            merged.contains("<!-- a - b -> c (frust plugin: camera) -->"),
            "{merged}"
        );
        assert_eq!(merged.matches("-->").count(), 1, "{merged}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A well-formed XML comment: delimited, with no `--` and no trailing `-`
    /// anywhere inside it. Anything else is a plist that does not parse.
    fn assert_well_formed_comment(line: &str) {
        let inner = line
            .trim()
            .strip_prefix("<!--")
            .and_then(|rest| rest.strip_suffix("-->"))
            .unwrap_or_else(|| panic!("not a delimited XML comment: {line:?}"));
        assert!(!inner.contains("--"), "{line:?}");
        assert!(!inner.ends_with('-'), "{line:?}");
    }

    /// Collapsing `--` to `-` once is not a fixed point (`---` leaves a `--`
    /// behind), and the plugin id is part of the same comment text — so the
    /// sanitization runs over the whole composed string, to a fixed point.
    #[test]
    fn every_run_of_dashes_in_a_comment_collapses_to_one() {
        for (comment, plugin_id) in [
            ("a --- b", "camera"),
            ("a ---- b", "camera"),
            ("plain", "cam--era"),
            ("trailing dash -", "camera"),
            ("--", "--"),
        ] {
            let line = comment_line(comment, plugin_id);
            assert_well_formed_comment(&line);
            assert!(line.starts_with('\t') && line.ends_with('\n'), "{line:?}");
        }
        // …and a run really does collapse to a single dash, rather than being
        // dropped: the text still reads.
        assert!(
            comment_line("a ---- b", "camera").contains("a - b (frust plugin: camera)"),
            "{}",
            comment_line("a ---- b", "camera")
        );
    }

    /// The group's last *content* line is the one definition
    /// [`linux::entry_value`]'s lookup uses: a comment block at the end of
    /// `[Desktop Entry]` introduces the group that follows it, so a
    /// contributed key belongs *above* it, after the last real `key=value`.
    #[test]
    fn a_contributed_key_lands_above_a_trailing_comment_block() {
        let dir = temp_dir("desktop-trailing-comment");
        let entry = dir.join("app.desktop");
        fs::write(
            &entry,
            "[Desktop Entry]\nType=Application\nName=My App\n\n\
             # A new window action:\n# (kept for the launcher menu)\n\
             [Desktop Action new]\nName=New Window\n",
        )
        .unwrap();

        merge_desktop_entry(&entry, &[row(&MIME_TYPE)], &mut Vec::new()).unwrap();

        let merged = fs::read_to_string(&entry).unwrap();
        let mime_at = merged.find("MimeType=image/png;").unwrap();
        assert!(mime_at > merged.find("Name=My App").unwrap(), "{merged}");
        assert!(
            mime_at < merged.find("# A new window action:").unwrap(),
            "{merged}"
        );
        // The comment block still introduces the group it was written for.
        assert!(
            merged.contains(
                "# A new window action:\n# (kept for the launcher menu)\n[Desktop Action new]"
            ),
            "{merged}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A file with no `[Desktop Entry]` group is the Linux counterpart of the
    /// anchorless plist: a refusal, not a silent skip.
    #[test]
    fn a_desktop_entry_without_its_group_refuses_the_build() {
        let dir = temp_dir("desktop-malformed");
        let entry = dir.join("app.desktop");
        fs::write(&entry, "# just a comment\n[Desktop Action new]\nName=New\n").unwrap();

        let err = merge_desktop_entry(&entry, &[row(&MIME_TYPE)], &mut Vec::new()).unwrap_err();
        assert!(
            matches!(err, DesktopBuildError::ContributionUnappliable { .. }),
            "{err}"
        );
        assert!(err.to_string().contains(DESKTOP_ENTRY_GROUP), "{err}");
        assert!(
            !fs::read_to_string(&entry).unwrap().contains("MimeType"),
            "the malformed entry was rewritten anyway"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
