//! Windows verbatim-path (`\\?\`) simplification and portable path
//! rendering, shared by the scaffold pipeline (`Cargo.toml`/
//! `gradle.properties`/the iOS `project.pbxproj` all reject or mis-resolve a
//! raw verbatim path or a bare backslash) and, for [`same_path`]/[`is_under`],
//! the TUI's recent-projects identity check.
//!
//! `Path::canonicalize()` on Windows returns a *verbatim* path
//! (`\\?\C:\...` or `\\?\UNC\server\share\...`). That form is exactly what
//! the filesystem wants, and exactly what breaks everything downstream of a
//! scaffold: a raw backslash inside a TOML basic string is an invalid escape
//! (`\C`, `\d`, ...), the same backslash inside a Java `.properties` value is
//! also an escape, and `..`-joining a verbatim path (the plugin-sibling walk)
//! never resolves because the verbatim form disables `..`/`.` normalization
//! by design.
//!
//! [`home_dir_from`] is this module's other shared seam: the one
//! `HOME`/`USERPROFILE`/`HOMEDRIVE`+`HOMEPATH` fallback chain, fed by
//! whatever environment-lookup abstraction each caller already has
//! (`frust-tui`'s `persist::EnvLookup`, `frust-dap`'s
//! `ide_config::vscode::EnvLookup`, `frust-drive`'s own
//! `doctor::EnvLookup`) — a pure function with no knowledge of any of those
//! traits, so it can sit below all three without a new dependency in either
//! direction.
//!
//! [`simplify`] strips the verbatim prefix back to the plain drive/UNC form,
//! but only when doing so is provably safe — the same rule the `dunce` crate
//! popularized: a plain path that names a reserved MS-DOS device
//! (`CON`/`NUL`/...), carries a trailing dot or space in a component, or
//! exceeds the classic 260-character `MAX_PATH` ceiling behaves differently
//! (or not at all) without the verbatim prefix, so those cases are left
//! verbatim rather than silently changed.
//!
//! Every public function is a thin wrapper around an inner, `windows:
//! bool`-parameterised implementation. The standard library's own verbatim-
//! prefix parser (`std::path::Component::Prefix`) is compiled out entirely on
//! a non-Windows target — `Path::components()` never yields a `Prefix`
//! variant there, and a literal backslash is just an ordinary filename
//! character, not a separator — so this module parses the Windows string
//! grammar itself rather than relying on it, which is what makes the
//! `windows: bool` arm exercisable (and tested) from any host, not just one
//! running the code for real.

use std::io;
use std::path::{Path, PathBuf};

/// The classic per-path length ceiling a plain (non-verbatim) Windows path is
/// still bound by; a path longer than this needs the verbatim prefix to work
/// at all, so it can never be safely de-verbatim-ized.
const MAX_PATH: usize = 260;

/// Legacy MS-DOS device names Windows reserves as a whole path component
/// regardless of any extension (`NUL` and `NUL.txt` are both reserved),
/// checked case-insensitively against the component's stem (the text before
/// its first `.`).
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The user's home directory, tried through every convention this
/// workspace's supported dev hosts use, in order: `HOME` (Unix), then
/// `USERPROFILE` (the usual Windows convention — `HOME` is normally unset
/// there), then `HOMEDRIVE`+`HOMEPATH` concatenated (the older Windows pair,
/// still set by some shells when `USERPROFILE` is not; `HOMEDRIVE` is a bare
/// drive like `C:` and `HOMEPATH` a drive-relative path like `\Users\ed`, so
/// this is plain string concatenation, not `Path::join` — joining a rooted
/// second component would discard the drive). An empty value is treated as
/// unset at every step, and the two Windows fallbacks are consulted
/// unconditionally rather than gated behind `cfg!(windows)`: those variables
/// are practically never set on Linux/macOS, so the extra fallback changes
/// nothing there, and staying unconditional is what makes the whole chain —
/// including its Windows-only tail — exercisable from a unit test on any
/// host.
///
/// `get` abstracts the actual environment-variable lookup so each caller
/// feeds its own seam (`frust-tui`'s `engine::persist::EnvLookup`,
/// `frust-dap`'s `ide_config::vscode::EnvLookup`, `frust-drive`'s own
/// `doctor::EnvLookup`) rather than this shared, pure resolver depending on
/// any of them, or reading the real, global process environment directly.
///
/// Returns `None` only when nothing in the chain resolves to a non-empty
/// value — a caller that needs a containment boundary (e.g. `frust-dap`'s
/// `detect_workspace_root`/`guard_workspace_root`) fails closed on that,
/// exactly as it did before this resolver was extracted.
pub fn home_dir_from(get: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let value = |key: &str| get(key).filter(|value| !value.is_empty());
    if let Some(home) = value("HOME") {
        return Some(PathBuf::from(home));
    }
    if let Some(profile) = value("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    let drive = value("HOMEDRIVE")?;
    let path = value("HOMEPATH")?;
    Some(PathBuf::from(format!("{drive}{path}")))
}

/// Strips a Windows verbatim prefix back to the plain drive/UNC form
/// (`\\?\C:\dev\x` -> `C:\dev\x`; `\\?\UNC\srv\sh\p` -> `\\srv\sh\p`) when
/// that is provably safe (see the module doc); identity otherwise, and
/// identity outright on a non-Windows host.
pub fn simplify(path: &Path) -> PathBuf {
    simplify_inner(path, cfg!(windows))
}

/// [`simplify`]'s implementation, parameterised on whether to apply Windows
/// verbatim-path semantics at all — always `cfg!(windows)` in production, an
/// explicit `true` in this module's own tests (and a same-crate test that
/// needs to exercise the Windows arm from a Linux host, e.g.
/// [`crate::scaffold::context`]'s scaffold-render tests) so the Windows arm
/// is provably correct on any host.
pub(crate) fn simplify_inner(path: &Path, windows: bool) -> PathBuf {
    if !windows {
        return path.to_path_buf();
    }
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    match de_verbatim(raw) {
        Some(plain) => PathBuf::from(plain),
        None => path.to_path_buf(),
    }
}

/// The de-verbatim'd plain form of `raw`, or `None` when `raw` carries no
/// verbatim prefix, or stripping it would not be safe (see the module doc) —
/// the caller then keeps the original, verbatim string unchanged.
fn de_verbatim(raw: &str) -> Option<String> {
    let plain = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        let rest = raw.strip_prefix(r"\\?\")?;
        // Only a plain drive path (`C:\...`) has a plain equivalent at all —
        // a volume-GUID or device-namespace verbatim path (`\\?\Volume{...}`)
        // does not, and must stay verbatim.
        let bytes = rest.as_bytes();
        let is_drive = bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes.len() == 2 || bytes[2] == b'\\');
        if !is_drive {
            return None;
        }
        rest.to_string()
    };

    is_safe_plain_path(&plain).then_some(plain)
}

/// Whether `plain` (an already de-verbatim'd path, no `\\?\` prefix) is safe
/// to hand back as-is: within [`MAX_PATH`], and no component is a reserved
/// device name or carries a trailing dot/space — each of those makes Windows
/// treat the plain form differently from the verbatim one it came from.
fn is_safe_plain_path(plain: &str) -> bool {
    if plain.len() > MAX_PATH {
        return false;
    }
    // Splitting on `\` and skipping empty segments handles the drive
    // (`C:\x`) and UNC (`\\server\share\x`) shapes uniformly — a UNC path's
    // two leading separators just produce a leading empty segment.
    for component in plain.split('\\') {
        if component.is_empty() {
            continue;
        }
        if component.ends_with('.') || component.ends_with(' ') {
            return false;
        }
        let stem = component.split('.').next().unwrap_or(component);
        if RESERVED_NAMES
            .iter()
            .any(|name| stem.eq_ignore_ascii_case(name))
        {
            return false;
        }
    }
    true
}

/// [`Path::canonicalize`] followed by [`simplify`] — the shape every
/// scaffold-time frust-path resolver needs: a real, existing, absolute path
/// with no verbatim prefix left in it for a template to bake into a TOML
/// string, a `.properties` value, or an Xcode `relativePath`.
pub fn canonicalize_simplified(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize().map(|canonical| simplify(&canonical))
}

/// Renders `path` for a build-tool consumer that accepts (or requires)
/// forward slashes: [`simplify`], then normalizes every remaining separator
/// to `/`. Cargo, Gradle, and Xcode all accept forward slashes on Windows,
/// and a forward slash needs no escaping in either a TOML basic string or a
/// Java `.properties` value — this is what actually fixes the unparseable
/// manifest, not just the verbatim prefix. Identity on Unix (no backslash
/// ever appears in a Unix path to begin with).
pub fn to_portable_string(path: &Path) -> String {
    to_portable_string_inner(path, cfg!(windows))
}

pub(crate) fn to_portable_string_inner(path: &Path, windows: bool) -> String {
    let simplified = simplify_inner(path, windows);
    let rendered = simplified.to_string_lossy().into_owned();
    if windows {
        rendered.replace('\\', "/")
    } else {
        rendered
    }
}

/// Whether `a` and `b` name the same location after [`simplify`] — on
/// Windows, component-wise and case-insensitive (`C:\Dev\X` == `c:/dev/x`,
/// and either separator is accepted since a value that already went through
/// [`to_portable_string`] uses `/`); on every other host, exact component
/// equality (so a trailing separator or a redundant `.` doesn't cause a
/// false negative, without attempting any case-folding a real filesystem
/// there wouldn't apply either).
pub fn same_path(a: &Path, b: &Path) -> bool {
    same_path_inner(a, b, cfg!(windows))
}

fn same_path_inner(a: &Path, b: &Path, windows: bool) -> bool {
    normalized_components(a, windows) == normalized_components(b, windows)
}

/// Whether `path` is `dir` or a descendant of it, by the same
/// simplify-then-compare rule [`same_path`] uses (case-insensitive
/// component-prefix match on Windows, exact component-prefix match
/// elsewhere).
pub fn is_under(path: &Path, dir: &Path) -> bool {
    is_under_inner(path, dir, cfg!(windows))
}

fn is_under_inner(path: &Path, dir: &Path, windows: bool) -> bool {
    let path_components = normalized_components(path, windows);
    let dir_components = normalized_components(dir, windows);
    dir_components.len() <= path_components.len()
        && path_components[..dir_components.len()] == dir_components[..]
}

/// The component list [`same_path`]/[`is_under`] compare: on Windows, split
/// on either separator and lower-cased (the standard library's own
/// `Path::components` can't be used here — it never splits on `\` at all
/// when not actually compiled for a Windows target); elsewhere, the real
/// `Path::components()` walk, so a trailing separator or `.` component
/// doesn't produce a spurious mismatch.
fn normalized_components(path: &Path, windows: bool) -> Vec<String> {
    let simplified = simplify_inner(path, windows);
    if windows {
        simplified
            .to_string_lossy()
            .split(['/', '\\'])
            .filter(|segment| !segment.is_empty() && *segment != ".")
            .map(|segment| segment.to_lowercase())
            .collect()
    } else {
        simplified
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory environment fixture for [`home_dir_from`]'s tests —
    /// built as a plain `HashMap` (rather than a trait object) since
    /// `home_dir_from` takes a closure, not an `EnvLookup`-shaped seam.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn home_dir_from_prefers_home_over_every_windows_fallback() {
        let get = env(&[
            ("HOME", "/home/ed"),
            ("USERPROFILE", r"C:\Users\ed"),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\Users\ed"),
        ]);
        assert_eq!(home_dir_from(get), Some(PathBuf::from("/home/ed")));
    }

    #[test]
    fn home_dir_from_falls_back_to_userprofile_when_home_unset() {
        let get = env(&[("USERPROFILE", r"C:\Users\cpu")]);
        assert_eq!(home_dir_from(get), Some(PathBuf::from(r"C:\Users\cpu")));
    }

    #[test]
    fn home_dir_from_falls_back_to_homedrive_and_homepath() {
        let get = env(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\cpu")]);
        assert_eq!(home_dir_from(get), Some(PathBuf::from(r"C:\Users\cpu")));
    }

    #[test]
    fn home_dir_from_treats_empty_values_as_unset() {
        let get = env(&[
            ("HOME", ""),
            ("USERPROFILE", ""),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\Users\cpu"),
        ]);
        assert_eq!(home_dir_from(get), Some(PathBuf::from(r"C:\Users\cpu")));
    }

    #[test]
    fn home_dir_from_none_when_nothing_resolves() {
        assert_eq!(home_dir_from(env(&[])), None);
        assert_eq!(home_dir_from(env(&[("HOMEDRIVE", "C:")])), None);
    }

    #[test]
    fn simplify_strips_verbatim_drive_prefix() {
        let simplified = simplify_inner(Path::new(r"\\?\C:\dev\x"), true);
        assert_eq!(simplified, PathBuf::from(r"C:\dev\x"));
    }

    #[test]
    fn simplify_strips_verbatim_unc_prefix() {
        let simplified = simplify_inner(Path::new(r"\\?\UNC\srv\sh\p"), true);
        assert_eq!(simplified, PathBuf::from(r"\\srv\sh\p"));
    }

    #[test]
    fn simplify_is_identity_off_windows() {
        let verbatim = Path::new(r"\\?\C:\dev\x");
        assert_eq!(simplify_inner(verbatim, false), verbatim.to_path_buf());
    }

    #[test]
    fn simplify_is_identity_for_a_plain_path() {
        let plain = Path::new(r"C:\dev\x");
        assert_eq!(simplify_inner(plain, true), plain.to_path_buf());
    }

    #[test]
    fn simplify_leaves_a_volume_guid_verbatim_path_untouched() {
        // No plain equivalent exists at all for a device-namespace verbatim
        // path, unlike a drive or UNC one.
        let verbatim = Path::new(r"\\?\Volume{9a6a5e21-0000-0000-0000-100000000000}\x");
        assert_eq!(simplify_inner(verbatim, true), verbatim.to_path_buf());
    }

    #[test]
    fn simplify_keeps_verbatim_when_a_component_is_a_reserved_device_name() {
        let verbatim = Path::new(r"\\?\C:\dev\CON\x");
        assert_eq!(simplify_inner(verbatim, true), verbatim.to_path_buf());

        // Reserved regardless of extension.
        let verbatim_with_ext = Path::new(r"\\?\C:\dev\NUL.txt");
        assert_eq!(
            simplify_inner(verbatim_with_ext, true),
            verbatim_with_ext.to_path_buf()
        );
    }

    #[test]
    fn simplify_keeps_verbatim_when_a_component_has_a_trailing_dot_or_space() {
        let trailing_dot = Path::new(r"\\?\C:\dev\pkg.\x");
        assert_eq!(
            simplify_inner(trailing_dot, true),
            trailing_dot.to_path_buf()
        );

        let trailing_space = Path::new(r"\\?\C:\dev\pkg \x");
        assert_eq!(
            simplify_inner(trailing_space, true),
            trailing_space.to_path_buf()
        );
    }

    #[test]
    fn simplify_keeps_verbatim_when_the_plain_path_exceeds_max_path() {
        let long_segment = "a".repeat(300);
        let verbatim = PathBuf::from(format!(r"\\?\C:\{long_segment}"));
        assert_eq!(simplify_inner(&verbatim, true), verbatim);
    }

    #[test]
    fn to_portable_string_renders_forward_slashes_for_a_windows_drive_path() {
        assert_eq!(
            to_portable_string_inner(Path::new(r"\\?\C:\dev\x"), true),
            "C:/dev/x",
        );
    }

    #[test]
    fn to_portable_string_renders_forward_slashes_for_a_windows_unc_path() {
        assert_eq!(
            to_portable_string_inner(Path::new(r"\\?\UNC\srv\sh\p"), true),
            "//srv/sh/p",
        );
    }

    #[test]
    fn to_portable_string_is_identity_on_unix() {
        assert_eq!(
            to_portable_string_inner(Path::new("/home/dev/frust/crates/frust"), false),
            "/home/dev/frust/crates/frust",
        );
    }

    #[test]
    fn same_path_drops_a_dot_component_on_windows() {
        // A `.` (current-dir) segment must not produce a spurious mismatch on
        // the Windows arm, exactly as the real `Path::components()` walk
        // already guarantees on the non-Windows arm — this is what makes
        // `frust-tui`'s `same_project_root` (a plain delegation to
        // `same_path`) behave identically to its own pre-`host_path`
        // `Path::components()`-based comparison on every platform.
        assert!(same_path_inner(
            Path::new(r"C:\dev\huddle"),
            Path::new(r"C:\dev\huddle\."),
            true,
        ));
    }

    #[test]
    fn same_path_case_folds_on_windows() {
        assert!(same_path_inner(
            Path::new(r"\\?\C:\Dev\Frust"),
            Path::new("c:/dev/frust"),
            true,
        ));
    }

    #[test]
    fn same_path_is_case_sensitive_on_unix() {
        assert!(!same_path_inner(
            Path::new("/home/Dev/frust"),
            Path::new("/home/dev/frust"),
            false,
        ));
        assert!(same_path_inner(
            Path::new("/home/dev/frust"),
            Path::new("/home/dev/frust"),
            false,
        ));
    }

    #[test]
    fn is_under_case_folds_on_windows() {
        assert!(is_under_inner(
            Path::new(r"\\?\C:\Dev\Frust\crates\frust"),
            Path::new("c:/dev/frust"),
            true,
        ));
        assert!(!is_under_inner(
            Path::new(r"\\?\C:\Dev\Other"),
            Path::new("c:/dev/frust"),
            true,
        ));
    }

    #[test]
    fn is_under_is_exact_on_unix() {
        assert!(is_under_inner(
            Path::new("/home/dev/frust/crates/frust"),
            Path::new("/home/dev/frust"),
            false,
        ));
        assert!(!is_under_inner(
            Path::new("/home/Dev/frust"),
            Path::new("/home/dev/frust"),
            false,
        ));
    }
}
