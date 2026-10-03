//! Merge-writes into the Android project's `.properties` files —
//! `android/local.properties` above all, the gitignored, machine-local file
//! the Gradle side reads with `java.util.Properties`.
//!
//! Two writers share it: [`write`] stamps `frust.versionName`/
//! `frust.versionCode` (the Flutter-canonical version plumbing the Gradle
//! template reads with defaults), and `crate::platform_wiring` keeps the
//! embedding and plugin module directories there. Both go through [`merge`],
//! which sets only the keys it is given: every other line — `sdk.dir`
//! written by Android Studio, comments, the other writer's keys, line
//! endings — is preserved byte for byte, and a file whose keys already hold
//! the wanted values is not rewritten at all.
//!
//! Values are written escaped the way `java.util.Properties.store` escapes
//! them (backslashes, the `=`/`:`/`#`/`!` specials, a leading space, control
//! and non-ASCII characters as `\uXXXX`), so a Windows path or a home
//! directory with an accent in it loads back unchanged whichever charset the
//! reading side assumes.

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, Result};

/// Merge-writes `frust.versionName = version_name` and
/// `frust.versionCode = version_code` into
/// `<android_dir>/local.properties`, preserving every other line (including
/// unrelated keys and comments) already present.
pub fn write(android_dir: &Path, version_name: &str, version_code: &str) -> Result<()> {
    let path = android_dir.join("local.properties");
    merge(
        &path,
        &[
            ("frust.versionName", version_name),
            ("frust.versionCode", version_code),
        ],
        Missing::Append,
    )
    .with_context(|| format!("writing `{}`", path.display()))?;
    Ok(())
}

/// What [`merge`] did with one key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// The key already held the value; nothing was written for it.
    Unchanged,
    /// The key now holds the value. `previous` is the value it replaced
    /// (unescaped), `None` when the key was appended.
    Set { previous: Option<String> },
    /// The key is absent and [`Missing::Skip`] left it so.
    Absent,
}

/// How [`merge`] treats a key the file does not carry yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Missing {
    /// Append `key=value` at the end of the file (creating the file).
    Append,
    /// Leave the key out — refresh only a key that is already there.
    Skip,
}

/// Sets each `(key, value)` of `entries` in the `.properties` file at
/// `path`, returning one [`Change`] per entry in order.
///
/// A key that is present has its **last** logical line rewritten in place
/// (the occurrence `java.util.Properties` honours), keeping that line's
/// ending; continuation lines belonging to it are folded into the
/// replacement. An absent key is appended or skipped per `missing`. The
/// file is written once, and only when some entry actually changed — a
/// missing file with nothing to append is not created.
pub(crate) fn merge(
    path: &Path,
    entries: &[(&str, &str)],
    missing: Missing,
) -> io::Result<Vec<Change>> {
    let existing = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let mut lines = logical_lines(&existing);
    let mut changes = Vec::with_capacity(entries.len());
    let mut appended = String::new();
    for (key, value) in entries {
        let escaped = escape_value(value);
        let found = lines
            .iter()
            .rposition(|line| line.entry.as_ref().is_some_and(|(k, _)| k == key));
        match found {
            Some(index) => {
                let line = &mut lines[index];
                let current = line.entry.as_ref().map(|(_, v)| v.clone());
                if current.as_deref() == Some(*value) {
                    changes.push(Change::Unchanged);
                } else {
                    line.text = format!("{key}={escaped}{}", line.ending);
                    line.entry = Some(((*key).to_string(), (*value).to_string()));
                    changes.push(Change::Set { previous: current });
                }
            }
            None if missing == Missing::Append => {
                appended.push_str(&format!("{key}={escaped}\n"));
                changes.push(Change::Set { previous: None });
            }
            None => changes.push(Change::Absent),
        }
    }

    if changes.iter().any(|c| matches!(c, Change::Set { .. })) {
        let mut out: String = lines.iter().map(|line| line.text.as_str()).collect();
        if !appended.is_empty() && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&appended);
        fs::write(path, out)?;
    }
    Ok(changes)
}

/// The unescaped value of `key` in the `.properties` file at `path` (its last
/// occurrence), `None` when the file or the key is absent.
pub(crate) fn read_value(path: &Path, key: &str) -> io::Result<Option<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    Ok(logical_lines(&text)
        .into_iter()
        .rev()
        .find_map(|line| line.entry.filter(|(k, _)| k == key).map(|(_, v)| v)))
}

/// One logical `.properties` line: its exact source text (continuation lines
/// and final line ending included), the ending alone, and — for a key/value
/// line — the unescaped key and value.
struct LogicalLine {
    text: String,
    ending: String,
    entry: Option<(String, String)>,
}

/// Splits `text` into logical lines: a physical line ending in an odd number
/// of backslashes continues onto the next one, except a comment or blank
/// line, which never continues (the `java.util.Properties.load` grammar).
fn logical_lines(text: &str) -> Vec<LogicalLine> {
    let mut out = Vec::new();
    let mut physical = text.split_inclusive('\n').peekable();
    while let Some(first) = physical.next() {
        let mut source = first.to_string();
        let mut joined = String::new();
        let mut body = strip_ending(first);
        let trimmed = body.trim_start_matches([' ', '\t', '\u{c}']);
        let is_comment = trimmed.is_empty() || trimmed.starts_with(['#', '!']);
        if !is_comment {
            while continues(body) {
                joined.push_str(&body[..body.len() - 1]);
                let Some(next) = physical.next() else {
                    body = "";
                    break;
                };
                source.push_str(next);
                body = strip_ending(next).trim_start_matches([' ', '\t', '\u{c}']);
            }
            joined.push_str(body);
        }
        let ending = source[strip_ending(&source).len()..].to_string();
        let entry = (!is_comment).then(|| parse_entry(&joined));
        out.push(LogicalLine {
            text: source,
            ending,
            entry,
        });
    }
    out
}

fn strip_ending(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

/// Whether a physical line ends in an odd number of backslashes.
fn continues(body: &str) -> bool {
    body.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

/// The unescaped `(key, value)` of one joined logical line: the key runs to
/// the first unescaped `=`, `:` or whitespace; whitespace and at most one
/// `=`/`:` separate it from the value.
fn parse_entry(line: &str) -> (String, String) {
    let line = line.trim_start_matches([' ', '\t', '\u{c}']);
    let mut key_end = line.len();
    let mut escaped = false;
    for (at, c) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if matches!(c, '=' | ':' | ' ' | '\t' | '\u{c}') {
            key_end = at;
            break;
        }
    }
    let key = &line[..key_end];
    let mut rest = line[key_end..].trim_start_matches([' ', '\t', '\u{c}']);
    if let Some(after) = rest.strip_prefix(['=', ':']) {
        rest = after.trim_start_matches([' ', '\t', '\u{c}']);
    }
    (unescape(key), unescape(rest))
}

/// Resolves `\uXXXX`, `\t`/`\n`/`\r`/`\f`, and `\<c>` (any other character
/// stands for itself) — `java.util.Properties`' escape grammar.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut units: Vec<u16> = Vec::new();
    let mut chars = text.chars().peekable();
    let flush = |units: &mut Vec<u16>, out: &mut String| {
        if !units.is_empty() {
            out.push_str(&String::from_utf16_lossy(units));
            units.clear();
        }
    };
    while let Some(c) = chars.next() {
        if c != '\\' {
            flush(&mut units, &mut out);
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                match u16::from_str_radix(&hex, 16) {
                    Ok(unit) if hex.len() == 4 => units.push(unit),
                    _ => {
                        flush(&mut units, &mut out);
                        out.push_str(&hex);
                    }
                }
            }
            other => {
                flush(&mut units, &mut out);
                match other {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('f') => out.push('\u{c}'),
                    Some(c) => out.push(c),
                    None => {}
                }
            }
        }
    }
    flush(&mut units, &mut out);
    out
}

/// `value` escaped as `java.util.Properties.store` writes a value.
pub(crate) fn escape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (at, c) in value.chars().enumerate() {
        match c {
            ' ' if at == 0 => out.push_str("\\ "),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{c}' => out.push_str("\\f"),
            '=' | ':' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            c if (' '..='~').contains(&c) => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-local-properties-test-{tag}-{}-{n}",
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
        assert!(content.contains("frust.versionName=1.2.3"), "{content}");
        assert!(content.contains("frust.versionCode=42"), "{content}");
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
        assert!(content.contains("frust.versionName=1.0"), "{content}");
        assert!(content.contains("frust.versionCode=1"), "{content}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrites_existing_frust_keys_in_place() {
        let dir = unique_temp_dir("overwrite");
        fs::write(
            dir.join("local.properties"),
            "sdk.dir=/sdk\nfrust.versionName=0.1\nfrust.versionCode=1\n",
        )
        .unwrap();
        write(&dir, "2.0", "7").unwrap();
        let content = fs::read_to_string(dir.join("local.properties")).unwrap();
        assert_eq!(
            content
                .lines()
                .filter(|l| l.starts_with("frust.versionName="))
                .count(),
            1,
            "{content}"
        );
        assert!(content.contains("frust.versionName=2.0"), "{content}");
        assert!(content.contains("frust.versionCode=7"), "{content}");
        assert!(!content.contains("0.1"), "{content}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The version stamp and the platform wiring share the file: each
    /// writer's keys survive the other's merge, and so do the line endings
    /// and comments Android Studio leaves behind.
    #[test]
    fn the_version_writer_keeps_wiring_keys_and_crlf_lines() {
        let dir = unique_temp_dir("shared");
        let path = dir.join("local.properties");
        let original = "## written by Android Studio\r\nsdk.dir=C\\:\\\\sdk\r\n\
                        frust.embedding.dir=/checkout/frust-embedding\n";
        fs::write(&path, original).unwrap();
        write(&dir, "1.0", "3").unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(
            content,
            format!("{original}frust.versionName=1.0\nfrust.versionCode=3\n")
        );
        assert_eq!(
            read_value(&path, "sdk.dir").unwrap().as_deref(),
            Some("C:\\sdk")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_reports_each_key_and_leaves_a_current_file_untouched() {
        let dir = unique_temp_dir("merge");
        let path = dir.join("local.properties");
        fs::write(&path, "a=1\nb = 2\n").unwrap();
        let changes = merge(
            &path,
            &[("a", "1"), ("b", "3"), ("c", "4")],
            Missing::Append,
        )
        .unwrap();
        assert_eq!(
            changes,
            [
                Change::Unchanged,
                Change::Set {
                    previous: Some("2".into())
                },
                Change::Set { previous: None },
            ]
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "a=1\nb=3\nc=4\n");

        let again = merge(
            &path,
            &[("a", "1"), ("b", "3"), ("c", "4")],
            Missing::Append,
        )
        .unwrap();
        assert!(again.iter().all(|c| *c == Change::Unchanged), "{again:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "a=1\nb=3\nc=4\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_refreshes_only_a_present_key_and_never_creates_the_file() {
        let dir = unique_temp_dir("skip");
        let path = dir.join("gradle.properties");
        assert_eq!(
            merge(&path, &[("k", "v")], Missing::Skip).unwrap(),
            [Change::Absent]
        );
        assert!(!path.exists());
        fs::write(&path, "# k=commented\nk=old\nother=1").unwrap();
        assert_eq!(
            merge(&path, &[("k", "new")], Missing::Skip).unwrap(),
            [Change::Set {
                previous: Some("old".into())
            }]
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# k=commented\nk=new\nother=1"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// `java.util.Properties` keeps the last occurrence of a key, so that is
    /// the one compared and rewritten; a continued value is replaced whole.
    #[test]
    fn the_last_occurrence_wins_and_continuations_are_folded() {
        let dir = unique_temp_dir("last");
        let path = dir.join("local.properties");
        fs::write(&path, "k=first\nk=/a/\\\n    b\nz=1\n").unwrap();
        assert_eq!(read_value(&path, "k").unwrap().as_deref(), Some("/a/b"));
        merge(&path, &[("k", "/c")], Missing::Append).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "k=first\nk=/c\nz=1\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn values_round_trip_through_the_properties_escapes() {
        for value in [
            "/plain/path",
            "C:/Users/me/frust",
            "C:\\Users\\me\\frust",
            " leading space",
            "a=b:c#d!e",
            "/Users/zoë/ünïcode/日本",
            "tab\there",
            "emoji 🦀",
        ] {
            let escaped = escape_value(value);
            assert!(escaped.is_ascii(), "{escaped}");
            assert_eq!(parse_entry(&format!("k={escaped}")).1, value, "{escaped}");
        }
        assert_eq!(escape_value("C:\\x"), "C\\:\\\\x");
        assert_eq!(escape_value("/a b"), "/a b");
        assert_eq!(escape_value("é"), "\\u00E9");
    }

    #[test]
    fn keys_are_parsed_with_every_separator_form() {
        assert_eq!(parse_entry("k=/a"), ("k".into(), "/a".into()));
        assert_eq!(parse_entry("  k = /a"), ("k".into(), "/a".into()));
        assert_eq!(parse_entry("k:/a"), ("k".into(), "/a".into()));
        assert_eq!(parse_entry("k /a"), ("k".into(), "/a".into()));
        assert_eq!(parse_entry("k="), ("k".into(), String::new()));
        assert_eq!(parse_entry("k\\=x=v"), ("k=x".into(), "v".into()));
        assert_eq!(parse_entry("ks=/a").0, "ks");
    }
}
