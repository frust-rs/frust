//! Writes `android/local.properties`' `forgekit.versionName`/
//! `forgekit.versionCode` keys (spec §12.5, PLAN.md decision 2), the
//! Flutter-canonical version-plumbing mechanism the Gradle template (task
//! 62) reads via `Properties`/`getProperty(...)` with defaults. This is a
//! **merge-write**: any other key already in the file (e.g. `sdk.dir`,
//! written by Android Studio/the Gradle wrapper) is preserved untouched.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Merge-writes `forgekit.versionName = version_name` and
/// `forgekit.versionCode = version_code` into
/// `<android_dir>/local.properties`, preserving every other line (including
/// unrelated keys and comments) already present.
pub fn write(android_dir: &Path, version_name: &str, version_code: &str) -> Result<()> {
    let path = android_dir.join("local.properties");
    let existing = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("reading `{}`", path.display()))?
    } else {
        String::new()
    };

    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    set_property(&mut lines, "forgekit.versionName", version_name);
    set_property(&mut lines, "forgekit.versionCode", version_code);

    let mut content = lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }

    fs::write(&path, content).with_context(|| format!("writing `{}`", path.display()))
}

/// Replaces the value of `key=` if a line already sets it, else appends a
/// new `key=value` line — a minimal `.properties` merge, not a full parser
/// (no escaping/continuation-line support; ForgeKit only ever writes its own
/// two flat keys here).
fn set_property(lines: &mut Vec<String>, key: &str, value: &str) {
    let prefix = format!("{key}=");
    if let Some(line) = lines.iter_mut().find(|line| line.starts_with(&prefix)) {
        *line = format!("{prefix}{value}");
    } else {
        lines.push(format!("{prefix}{value}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-local-properties-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_both_keys_when_file_absent() {
        let dir = unique_temp_dir("absent");
        write(&dir, "1.2.3", "42").unwrap();
        let content = fs::read_to_string(dir.join("local.properties")).unwrap();
        assert!(content.contains("forgekit.versionName=1.2.3"), "{content}");
        assert!(content.contains("forgekit.versionCode=42"), "{content}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preserves_unrelated_keys_when_file_exists() {
        let dir = unique_temp_dir("preserve");
        fs::write(
            dir.join("local.properties"),
            "sdk.dir=/Users/me/Library/Android/sdk\nndk.dir=/some/ndk\n",
        )
        .unwrap();
        write(&dir, "1.0", "1").unwrap();
        let content = fs::read_to_string(dir.join("local.properties")).unwrap();
        assert!(
            content.contains("sdk.dir=/Users/me/Library/Android/sdk"),
            "{content}"
        );
        assert!(content.contains("ndk.dir=/some/ndk"), "{content}");
        assert!(content.contains("forgekit.versionName=1.0"), "{content}");
        assert!(content.contains("forgekit.versionCode=1"), "{content}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrites_existing_forgekit_keys_in_place() {
        let dir = unique_temp_dir("overwrite");
        fs::write(
            dir.join("local.properties"),
            "sdk.dir=/sdk\nforgekit.versionName=0.1\nforgekit.versionCode=1\n",
        )
        .unwrap();
        write(&dir, "2.0", "7").unwrap();
        let content = fs::read_to_string(dir.join("local.properties")).unwrap();
        assert_eq!(
            content
                .lines()
                .filter(|l| l.starts_with("forgekit.versionName="))
                .count(),
            1,
            "{content}"
        );
        assert!(content.contains("forgekit.versionName=2.0"), "{content}");
        assert!(content.contains("forgekit.versionCode=7"), "{content}");
        assert!(!content.contains("0.1"), "{content}");
        let _ = fs::remove_dir_all(&dir);
    }
}
