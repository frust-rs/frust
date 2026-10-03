//! Points a generated project's Android and iOS host projects at the
//! embedding modules shipped inside the shell crates cargo resolves for it.
//!
//! The Android embedding is a Gradle library module in
//! `frust-shell-android/platform/android/frust-embedding`, the iOS one a Swift
//! package in `frust-shell-ios/platform/ios/FrustEmbedding`. Where those
//! crates live depends on how the project depends on the framework — a
//! checkout's crate directory under `--frust-path`, the unpacked release under
//! cargo's registry cache otherwise — and moves whenever the resolved version
//! does (`cargo update`). So neither location is rendered at scaffold time:
//! [`sync`] asks [`crate::packages`] and writes the answer into the two places
//! the host build tools read it from:
//!
//! - `android/gradle.properties`' `frust.embedding.dir` (the absolute module
//!   directory; `settings.gradle.kts` includes it as `:frust-embedding`);
//! - the `ios/FrustEmbedding` symlink next to `Runner.xcodeproj`, which the
//!   project's local Swift package reference names by the relative path
//!   `FrustEmbedding`.
//!
//! Idempotent: an up-to-date project is left byte-identical, and the returned
//! [`Report`] says per platform what (if anything) changed. A project without
//! an `android/` or `ios/` directory is skipped for that platform, and cargo is
//! not asked about a platform that is skipped.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::host_path;
use crate::packages::{CargoLocator, PackageLocator, PackagesError};
use crate::process::RealProcessRunner;

/// The package shipping the Android embedding module.
pub const ANDROID_SHELL_PACKAGE: &str = "frust-shell-android";
/// The package shipping the iOS embedding Swift package.
pub const IOS_SHELL_PACKAGE: &str = "frust-shell-ios";
/// The Android embedding module's directory inside [`ANDROID_SHELL_PACKAGE`].
pub const ANDROID_EMBEDDING_REL: &str = "platform/android/frust-embedding";
/// The iOS embedding package's directory inside [`IOS_SHELL_PACKAGE`].
pub const IOS_EMBEDDING_REL: &str = "platform/ios/FrustEmbedding";
/// The `android/gradle.properties` key `settings.gradle.kts` reads the
/// embedding module's directory from.
pub const EMBEDDING_DIR_KEY: &str = "frust.embedding.dir";
/// The value the app template renders for [`EMBEDDING_DIR_KEY`] until the
/// first [`sync`]: a path that does not exist, phrased so the Gradle error a
/// sync-less project hits names the fix.
pub const UNRESOLVED_EMBEDDING_DIR: &str =
    "unresolved--run-frust-build-or-frust-run-in-the-project-directory";
/// The project-relative path of the Android properties file [`sync`] edits.
pub const GRADLE_PROPERTIES: &str = "android/gradle.properties";
/// The project-relative path of the symlink the iOS project's local package
/// reference (`relativePath = "FrustEmbedding"`) resolves through.
pub const IOS_EMBEDDING_LINK: &str = "ios/FrustEmbedding";

/// Why the wiring could not be brought up to date.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// cargo could not say where a shell crate lives.
    #[error("could not locate the frust shell crates: {0}")]
    Locate(#[from] PackagesError),
    /// The located shell crate does not carry its embedding — an incomplete
    /// package, which Gradle or Xcode would otherwise report far less clearly.
    #[error(
        "`{package}` resolved to '{}', which has no embedding at '{}'",
        package_dir.display(),
        embedding_dir.display()
    )]
    EmbeddingMissing {
        package: &'static str,
        package_dir: PathBuf,
        embedding_dir: PathBuf,
    },
    /// Reading or writing a project file failed.
    #[error("{action} '{}': {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

/// What [`sync`] did for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wiring {
    /// The project has no directory for this platform.
    Skipped,
    /// Already pointed at `target`; nothing was written.
    Unchanged { target: PathBuf },
    /// Now points at `target`. `previous` is the value or link target it
    /// replaced, `None` when the key or link did not exist yet.
    Updated {
        target: PathBuf,
        previous: Option<String>,
    },
    /// `path` exists but is not a symlink (a vendored copy, say) — user
    /// content, so it is left alone.
    Occupied { path: PathBuf, target: PathBuf },
    /// This host could not create the symlink (no symlink support, or a
    /// Windows account without the privilege). The rest of the sync stands.
    Unsupported { target: PathBuf, reason: String },
}

impl Wiring {
    /// Whether this platform's wiring was written.
    pub fn changed(&self) -> bool {
        matches!(self, Self::Updated { .. })
    }
}

/// The outcome of one [`sync`], per platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub android: Wiring,
    pub ios: Wiring,
}

impl Report {
    /// Whether any platform's wiring was written.
    pub fn changed(&self) -> bool {
        self.android.changed() || self.ios.changed()
    }

    /// One human-readable line per platform that was written or needs
    /// attention; empty when nothing changed and nothing is wrong. Skipped
    /// and unchanged platforms say nothing.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(line) = describe("Android", GRADLE_PROPERTIES, &self.android) {
            lines.push(line);
        }
        if let Some(line) = describe("iOS", IOS_EMBEDDING_LINK, &self.ios) {
            lines.push(line);
        }
        lines
    }
}

fn describe(platform: &str, file: &str, wiring: &Wiring) -> Option<String> {
    match wiring {
        Wiring::Skipped | Wiring::Unchanged { .. } => None,
        Wiring::Updated { target, .. } => Some(format!(
            "{platform} embedding: {file} -> {}",
            target.display()
        )),
        Wiring::Occupied { path, target } => Some(format!(
            "{platform} embedding: '{}' is not a symlink, left as is (expected a link to '{}')",
            path.display(),
            target.display()
        )),
        Wiring::Unsupported { target, reason } => Some(format!(
            "{platform} embedding: could not link {file} -> '{}': {reason}",
            target.display()
        )),
    }
}

/// Where `project_dir`'s Android and iOS embeddings resolve, without writing
/// anything — what [`sync`] would point the project at. Both are located
/// whether or not the project has the platform's directory, so a report (the
/// CLI's `doctor`) can show them for any project.
pub fn resolve(
    locator: &dyn PackageLocator,
    project_dir: &Path,
) -> Result<EmbeddingDirs, SyncError> {
    let dirs = locator.locate_many(project_dir, &[ANDROID_SHELL_PACKAGE, IOS_SHELL_PACKAGE])?;
    let [android, ios]: [PathBuf; 2] =
        dirs.try_into()
            .map_err(|dirs: Vec<PathBuf>| PackagesError::MetadataUnreadable {
                project_dir: project_dir.to_path_buf(),
                reason: format!("expected 2 package directories, got {}", dirs.len()),
            })?;
    Ok(EmbeddingDirs {
        android: embedding_dir(ANDROID_SHELL_PACKAGE, &android, ANDROID_EMBEDDING_REL)?,
        ios: embedding_dir(IOS_SHELL_PACKAGE, &ios, IOS_EMBEDDING_REL)?,
    })
}

/// The embedding directories [`resolve`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingDirs {
    /// The Android embedding Gradle module.
    pub android: PathBuf,
    /// The iOS embedding Swift package.
    pub ios: PathBuf,
}

/// Brings `project_dir`'s Android and iOS wiring up to date with the shell
/// crates `cargo metadata` resolves for it (`--locked` when the project has a
/// `Cargo.lock`).
pub fn sync(project_dir: &Path) -> Result<Report, SyncError> {
    sync_with(&CargoLocator::new(&RealProcessRunner), project_dir)
}

/// [`sync`] through an injected locator.
pub fn sync_with(locator: &dyn PackageLocator, project_dir: &Path) -> Result<Report, SyncError> {
    let has_android = project_dir.join("android").is_dir();
    let has_ios = project_dir.join("ios").is_dir();

    let mut wanted = Vec::new();
    if has_android {
        wanted.push((ANDROID_SHELL_PACKAGE, ANDROID_EMBEDDING_REL));
    }
    if has_ios {
        wanted.push((IOS_SHELL_PACKAGE, IOS_EMBEDDING_REL));
    }
    if wanted.is_empty() {
        return Ok(Report {
            android: Wiring::Skipped,
            ios: Wiring::Skipped,
        });
    }

    let names: Vec<&str> = wanted.iter().map(|(package, _)| *package).collect();
    let package_dirs = locator.locate_many(project_dir, &names)?;
    let mut embeddings = Vec::with_capacity(wanted.len());
    for ((package, rel), package_dir) in wanted.iter().zip(package_dirs) {
        embeddings.push(embedding_dir(package, &package_dir, rel)?);
    }
    let mut embeddings = embeddings.into_iter();

    let android = if has_android {
        let target = embeddings.next().expect("one directory per wanted package");
        write_gradle_property(&project_dir.join(GRADLE_PROPERTIES), &target)?
    } else {
        Wiring::Skipped
    };
    let ios = if has_ios {
        let target = embeddings.next().expect("one directory per wanted package");
        link_embedding(&project_dir.join(IOS_EMBEDDING_LINK), &target)?
    } else {
        Wiring::Skipped
    };
    Ok(Report { android, ios })
}

/// `rel` under `package_dir`, verified to exist and made absolute and
/// canonical, so the value written does not depend on the directory a build
/// tool resolves it from.
fn embedding_dir(
    package: &'static str,
    package_dir: &Path,
    rel: &str,
) -> Result<PathBuf, SyncError> {
    let dir = package_dir.join(rel);
    if !dir.is_dir() {
        return Err(SyncError::EmbeddingMissing {
            package,
            package_dir: package_dir.to_path_buf(),
            embedding_dir: dir,
        });
    }
    Ok(host_path::canonicalize_simplified(&dir).unwrap_or(dir))
}

/// Sets [`EMBEDDING_DIR_KEY`] in the properties file at `path` to `target`,
/// replacing the first uncommented occurrence of the key in place (its line
/// ending kept, every other line untouched) or appending it when absent.
fn write_gradle_property(path: &Path, target: &Path) -> Result<Wiring, SyncError> {
    let value = host_path::to_portable_string(target);
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(SyncError::Io {
                action: "reading",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut previous = None;
    let mut rewritten = String::with_capacity(existing.len() + value.len());
    for line in existing.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let ending = &line[body.len()..];
        match property_value(body) {
            Some(current) if previous.is_none() => {
                if current == value {
                    return Ok(Wiring::Unchanged {
                        target: target.to_path_buf(),
                    });
                }
                previous = Some(current.to_string());
                rewritten.push_str(&format!("{EMBEDDING_DIR_KEY}={value}{ending}"));
            }
            _ => rewritten.push_str(line),
        }
    }
    if previous.is_none() {
        if !rewritten.is_empty() && !rewritten.ends_with('\n') {
            rewritten.push('\n');
        }
        rewritten.push_str(&format!("{EMBEDDING_DIR_KEY}={value}\n"));
    }

    fs::write(path, rewritten).map_err(|source| SyncError::Io {
        action: "writing",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Wiring::Updated {
        target: target.to_path_buf(),
        previous,
    })
}

/// The value of an uncommented [`EMBEDDING_DIR_KEY`] line, `None` for any
/// other line. Recognises the `.properties` separators Gradle accepts (`=`,
/// `:`, or whitespace) around the key.
fn property_value(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(EMBEDDING_DIR_KEY)?;
    let separated = rest.trim_start_matches([' ', '\t']);
    let value = match separated.chars().next() {
        Some('=') | Some(':') => &separated[1..],
        _ if separated.len() < rest.len() || rest.is_empty() => separated,
        _ => return None,
    };
    Some(value.trim())
}

/// Points the symlink at `link` to `target`: created when absent, replaced
/// when it points elsewhere, left alone when it already points there or when
/// `link` is a real file or directory.
fn link_embedding(link: &Path, target: &Path) -> Result<Wiring, SyncError> {
    let previous = match fs::symlink_metadata(link) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(SyncError::Io {
                action: "inspecting",
                path: link.to_path_buf(),
                source,
            });
        }
        Ok(meta) if !meta.file_type().is_symlink() => {
            return Ok(Wiring::Occupied {
                path: link.to_path_buf(),
                target: target.to_path_buf(),
            });
        }
        Ok(_) => {
            let current = fs::read_link(link).map_err(|source| SyncError::Io {
                action: "reading the symlink",
                path: link.to_path_buf(),
                source,
            })?;
            if current == target {
                return Ok(Wiring::Unchanged {
                    target: target.to_path_buf(),
                });
            }
            remove_link(link).map_err(|source| SyncError::Io {
                action: "replacing the symlink",
                path: link.to_path_buf(),
                source,
            })?;
            Some(current.display().to_string())
        }
    };

    match create_dir_link(target, link) {
        Ok(()) => Ok(Wiring::Updated {
            target: target.to_path_buf(),
            previous,
        }),
        Err(err) if is_unsupported(&err) => Ok(Wiring::Unsupported {
            target: target.to_path_buf(),
            reason: err.to_string(),
        }),
        Err(source) => Err(SyncError::Io {
            action: "creating the symlink",
            path: link.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(not(any(unix, windows)))]
fn create_dir_link(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this platform has no symlinks",
    ))
}

/// A directory symlink is removed as a directory on Windows and as a file
/// everywhere else.
fn remove_link(link: &Path) -> io::Result<()> {
    if cfg!(windows) {
        fs::remove_dir(link).or_else(|_| fs::remove_file(link))
    } else {
        fs::remove_file(link)
    }
}

/// Whether a symlink-creation failure means "this host cannot do it" rather
/// than a fault in the project: no symlink support at all, or Windows'
/// `ERROR_PRIVILEGE_NOT_HELD` (1314) for an account without Developer Mode.
fn is_unsupported(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Unsupported || (cfg!(windows) && err.raw_os_error() == Some(1314))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::StubLocator;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-platform-wiring-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        host_path::canonicalize_simplified(&dir).unwrap()
    }

    /// A fake checkout holding both shell crates with their embeddings, plus
    /// the stub answering for it.
    fn checkout(root: &Path) -> (PathBuf, PathBuf, StubLocator) {
        let android = root.join("crates/frust-shell-android");
        let ios = root.join("crates/frust-shell-ios");
        fs::create_dir_all(android.join(ANDROID_EMBEDDING_REL)).unwrap();
        fs::create_dir_all(ios.join(IOS_EMBEDDING_REL)).unwrap();
        let stub = StubLocator::new()
            .with(ANDROID_SHELL_PACKAGE, &android)
            .with(IOS_SHELL_PACKAGE, &ios);
        (android, ios, stub)
    }

    const PROPERTIES: &str = "org.gradle.jvmargs=-Xmx2048m\r\nandroid.useAndroidX=true\n\n\
        # frust.embedding.dir=commented/out\nfrust.embedding.dir=placeholder\n";

    /// A project with `android/` (holding [`PROPERTIES`]) and `ios/`.
    fn project(root: &Path) -> PathBuf {
        let project = root.join("app");
        fs::create_dir_all(project.join("android")).unwrap();
        fs::create_dir_all(project.join("ios/Runner.xcodeproj")).unwrap();
        fs::write(project.join(GRADLE_PROPERTIES), PROPERTIES).unwrap();
        project
    }

    #[cfg(unix)]
    #[test]
    fn sync_rewrites_the_property_value_and_links_the_ios_package() {
        let root = temp_dir("fresh");
        let (android, ios, stub) = checkout(&root);
        let project = project(&root);

        let report = sync_with(&stub, &project).unwrap();
        let android_target = android.join(ANDROID_EMBEDDING_REL);
        let ios_target = ios.join(IOS_EMBEDDING_REL);
        assert_eq!(
            report.android,
            Wiring::Updated {
                target: android_target.clone(),
                previous: Some("placeholder".into()),
            }
        );
        assert_eq!(
            report.ios,
            Wiring::Updated {
                target: ios_target.clone(),
                previous: None,
            }
        );
        assert!(report.changed());

        // Only the uncommented key's value moved; every other byte, CRLF
        // ending included, is as it was.
        let properties = fs::read_to_string(project.join(GRADLE_PROPERTIES)).unwrap();
        assert_eq!(
            properties,
            PROPERTIES.replace(
                "frust.embedding.dir=placeholder",
                &format!("frust.embedding.dir={}", android_target.display())
            )
        );
        assert_eq!(
            fs::read_link(project.join(IOS_EMBEDDING_LINK)).unwrap(),
            ios_target
        );

        let lines = report.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("Android embedding: android/gradle.properties -> "));
        assert!(lines[1].starts_with("iOS embedding: ios/FrustEmbedding -> "));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_sync_changes_nothing() {
        let root = temp_dir("idempotent");
        let (_, _, stub) = checkout(&root);
        let project = project(&root);
        sync_with(&stub, &project).unwrap();
        let before = fs::read(project.join(GRADLE_PROPERTIES)).unwrap();

        let report = sync_with(&stub, &project).unwrap();
        assert!(
            matches!(report.android, Wiring::Unchanged { .. }),
            "{report:?}"
        );
        assert!(matches!(report.ios, Wiring::Unchanged { .. }), "{report:?}");
        assert!(!report.changed());
        assert!(report.lines().is_empty());
        assert_eq!(fs::read(project.join(GRADLE_PROPERTIES)).unwrap(), before);
        let _ = fs::remove_dir_all(&root);
    }

    /// What `cargo update` does to a registry project: the shell crates
    /// resolve somewhere new, and the next sync follows them.
    #[cfg(unix)]
    #[test]
    fn a_moved_package_replaces_the_value_and_the_link() {
        let root = temp_dir("moved");
        let (_, old_ios, stub) = checkout(&root.join("old"));
        let project = project(&root);
        sync_with(&stub, &project).unwrap();

        let (android, ios, moved) = checkout(&root.join("new"));
        let report = sync_with(&moved, &project).unwrap();
        assert_eq!(
            report.ios,
            Wiring::Updated {
                target: ios.join(IOS_EMBEDDING_REL),
                previous: Some(old_ios.join(IOS_EMBEDDING_REL).display().to_string()),
            }
        );
        assert!(report.android.changed());
        assert_eq!(
            fs::read_link(project.join(IOS_EMBEDDING_LINK)).unwrap(),
            ios.join(IOS_EMBEDDING_REL)
        );
        let properties = fs::read_to_string(project.join(GRADLE_PROPERTIES)).unwrap();
        assert!(
            properties.contains(&format!(
                "frust.embedding.dir={}\n",
                android.join(ANDROID_EMBEDDING_REL).display()
            )),
            "{properties}"
        );
        assert_eq!(properties.matches("\nfrust.embedding.dir=").count(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_key_is_appended_and_a_missing_file_created() {
        let root = temp_dir("append");
        let (android, _, stub) = checkout(&root);
        let project = root.join("app");
        fs::create_dir_all(project.join("android")).unwrap();
        fs::write(project.join(GRADLE_PROPERTIES), "android.useAndroidX=true").unwrap();

        let report = sync_with(&stub, &project).unwrap();
        assert_eq!(
            report.android,
            Wiring::Updated {
                target: android.join(ANDROID_EMBEDDING_REL),
                previous: None,
            }
        );
        assert_eq!(
            fs::read_to_string(project.join(GRADLE_PROPERTIES)).unwrap(),
            format!(
                "android.useAndroidX=true\nfrust.embedding.dir={}\n",
                android.join(ANDROID_EMBEDDING_REL).display()
            )
        );

        fs::remove_file(project.join(GRADLE_PROPERTIES)).unwrap();
        assert!(sync_with(&stub, &project).unwrap().android.changed());
        assert!(
            fs::read_to_string(project.join(GRADLE_PROPERTIES))
                .unwrap()
                .starts_with("frust.embedding.dir=")
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A platform directory that is absent is skipped, and cargo is not asked
    /// about its package — a project with neither never reaches the locator.
    #[test]
    fn absent_platform_directories_are_skipped_without_locating() {
        let root = temp_dir("skipped");
        let project = root.join("desktop_only");
        fs::create_dir_all(&project).unwrap();
        let failing = StubLocator::failing("must not be asked");
        let report = sync_with(&failing, &project).unwrap();
        assert_eq!(report.android, Wiring::Skipped);
        assert_eq!(report.ios, Wiring::Skipped);
        assert_eq!(failing.calls(), 0);

        // Android only: the iOS package need not even be resolvable.
        fs::create_dir_all(project.join("android")).unwrap();
        let android_dir = root.join("crates/frust-shell-android");
        fs::create_dir_all(android_dir.join(ANDROID_EMBEDDING_REL)).unwrap();
        let android_only = StubLocator::new().with(ANDROID_SHELL_PACKAGE, &android_dir);
        let report = sync_with(&android_only, &project).unwrap();
        assert!(report.android.changed());
        assert_eq!(report.ios, Wiring::Skipped);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn locate_failures_and_incomplete_packages_are_errors() {
        let root = temp_dir("errors");
        let project = project(&root);
        let err = sync_with(&StubLocator::failing("offline"), &project).unwrap_err();
        assert!(matches!(err, SyncError::Locate(_)), "{err:?}");
        assert!(err.to_string().contains("offline"), "{err}");

        let bare = root.join("bare");
        fs::create_dir_all(&bare).unwrap();
        let stub = StubLocator::new()
            .with(ANDROID_SHELL_PACKAGE, &bare)
            .with(IOS_SHELL_PACKAGE, &bare);
        let err = sync_with(&stub, &project).unwrap_err();
        assert!(
            matches!(err, SyncError::EmbeddingMissing { package, .. } if package == ANDROID_SHELL_PACKAGE),
            "{err:?}"
        );
        assert_eq!(
            fs::read_to_string(project.join(GRADLE_PROPERTIES)).unwrap(),
            PROPERTIES,
            "a failed sync writes nothing"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A real directory where the link belongs is user content — reported,
    /// never removed.
    #[test]
    fn a_real_directory_in_place_of_the_link_is_left_alone() {
        let root = temp_dir("occupied");
        let (_, _, stub) = checkout(&root);
        let project = project(&root);
        fs::create_dir_all(project.join(IOS_EMBEDDING_LINK).join("Sources")).unwrap();

        let report = sync_with(&stub, &project).unwrap();
        assert!(matches!(report.ios, Wiring::Occupied { .. }), "{report:?}");
        assert!(project.join(IOS_EMBEDDING_LINK).join("Sources").is_dir());
        assert!(report.lines()[1].contains("is not a symlink"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_reports_both_embeddings_without_writing() {
        let root = temp_dir("resolve");
        let (android, ios, stub) = checkout(&root);
        let project = root.join("no_platform_dirs");
        fs::create_dir_all(&project).unwrap();
        assert_eq!(
            resolve(&stub, &project).unwrap(),
            EmbeddingDirs {
                android: android.join(ANDROID_EMBEDDING_REL),
                ios: ios.join(IOS_EMBEDDING_REL),
            }
        );
        assert!(!project.join("android").exists() && !project.join("ios").exists());
        assert!(matches!(
            resolve(&StubLocator::failing("offline"), &project),
            Err(SyncError::Locate(_))
        ));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn property_lines_are_matched_by_key_not_prefix() {
        assert_eq!(property_value("frust.embedding.dir=/a"), Some("/a"));
        assert_eq!(property_value("  frust.embedding.dir = /a "), Some("/a"));
        assert_eq!(property_value("frust.embedding.dir:/a"), Some("/a"));
        assert_eq!(property_value("frust.embedding.dir /a"), Some("/a"));
        assert_eq!(property_value("frust.embedding.dir="), Some(""));
        assert_eq!(property_value("frust.embedding.dirs=/a"), None);
        assert_eq!(property_value("# frust.embedding.dir=/a"), None);
        assert_eq!(property_value("android.useAndroidX=true"), None);
    }
}
