//! `DEVELOPMENT_TEAM` resolution for signed iOS builds.
//!
//! Precedence: `FRUST_IOS_TEAM` env → `[ios] team` in `frust.toml` →
//! auto-detect from `security find-identity -v -p codesigning`. Zero detected
//! teams while signing is required is an actionable error; multiple picks the
//! first and prints how to override.
//!
//! Auto-detection reads each identity's team from its certificate's subject
//! `OU` (`security find-certificate -c <label>`), **not** from the
//! parenthesized id in the certificate name: for a *distribution* certificate
//! the two coincide, but an *Apple Development* certificate issued to a
//! member of an organization team carries the developer's own id in the name
//! and the team in the `OU` — handing the name's id to `xcodebuild` as
//! `DEVELOPMENT_TEAM` fails with `No Account for Team`. The name's id is kept
//! only as the fallback when the certificate cannot be read.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::doctor::EnvLookup;
use crate::process::ProcessRunner;

/// The env var that overrides team resolution outright.
const TEAM_ENV: &str = "FRUST_IOS_TEAM";

/// A resolved development team plus an optional note to print (e.g. the
/// "multiple teams found, using X" disambiguation message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamChoice {
    pub team: String,
    pub note: Option<String>,
}

/// Reads only the `[ios] team` field of `frust.toml` — a local, additive
/// parse so this doesn't touch `ios_run::project`'s own struct. serde ignores
/// the other `[app]`/`[ios]` fields.
#[derive(Debug, Deserialize)]
struct TeamToml {
    #[serde(default)]
    ios: Option<IosTeam>,
}

#[derive(Debug, Deserialize)]
struct IosTeam {
    #[serde(default)]
    team: Option<String>,
}

/// Resolves the `DEVELOPMENT_TEAM` for a signed build, applying the
/// env → toml → auto-detect precedence.
pub fn resolve(
    env: &dyn EnvLookup,
    runner: &dyn ProcessRunner,
    project_dir: &Path,
) -> Result<TeamChoice> {
    if let Some(team) = env.get(TEAM_ENV).and_then(non_empty) {
        return Ok(TeamChoice { team, note: None });
    }
    if let Some(team) = team_from_toml(project_dir)? {
        return Ok(TeamChoice { team, note: None });
    }

    let teams = detect(runner)?;
    match teams.as_slice() {
        [] => bail!(
            "no codesigning identity found. Open Xcode → Settings → Accounts and \
             add your Apple ID, or pass --no-codesign for an unsigned build."
        ),
        [only] => Ok(TeamChoice {
            team: only.clone(),
            note: None,
        }),
        [first, ..] => Ok(TeamChoice {
            team: first.clone(),
            note: Some(format!(
                "multiple signing teams found ({}); using `{first}`. Override with \
                 FRUST_IOS_TEAM=<id> or `[ios] team` in frust.toml.",
                teams.join(", ")
            )),
        }),
    }
}

/// Runs `security find-identity -v -p codesigning` and resolves the distinct
/// team ids of its identity lines, in first-seen order: each identity's
/// certificate `OU` ([`team_from_certificate`]), falling back to the id in
/// the certificate name ([`extract_team`]) when the certificate cannot be
/// read — see the module doc for why the `OU` is the authority.
pub fn detect(runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    let out = runner
        .run("security", &["find-identity", "-v", "-p", "codesigning"])
        .context("running `security find-identity`")?;
    let mut teams: Vec<String> = Vec::new();
    for line in out.stdout.lines() {
        let team = identity_label(line)
            .and_then(|label| team_from_certificate(runner, label))
            .or_else(|| extract_team(line));
        if let Some(team) = team
            && !teams.contains(&team)
        {
            teams.push(team);
        }
    }
    Ok(teams)
}

/// The quoted certificate label of a `security find-identity` identity line
/// (`  1) <hash> "Apple Development: Ada (ABCDEFGHIJ)"`), `None` for the
/// summary line and anything else unquoted.
fn identity_label(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = line[start..].rfind('"')? + start;
    (end > start).then(|| &line[start..end])
}

/// The team id in the subject `OU` of the certificate labelled `label`:
/// `security find-certificate -c <label>` prints the subject as a DER hex
/// blob (`"subj"<blob>=0x…`), and the first `organizationalUnitName` whose
/// value is ten `[A-Z0-9]` characters is the team. `None` when the lookup
/// fails or no such `OU` exists.
fn team_from_certificate(runner: &dyn ProcessRunner, label: &str) -> Option<String> {
    let out = runner
        .run("security", &["find-certificate", "-c", label])
        .ok()?;
    if !out.success {
        return None;
    }
    subject_team(&out.stdout)
}

/// The team id from a `security find-certificate` attribute dump — its
/// `"subj"<blob>=0x…` line (hex, then a quoted ASCII rendering) decoded and
/// scanned by [`team_from_subject_der`].
fn subject_team(find_certificate_output: &str) -> Option<String> {
    find_certificate_output.lines().find_map(|line| {
        // The hex is followed by two spaces and a quoted ASCII rendering of
        // the same bytes; only the first token is the DER.
        let rest = line.trim().strip_prefix("\"subj\"<blob>=0x")?;
        let der = decode_hex(rest.split_whitespace().next()?)?;
        team_from_subject_der(&der)
    })
}

/// Decodes an even-length string of hex digit pairs; `None` on any other
/// input.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The first `OU` (OID `2.5.4.11`, DER `06 03 55 04 0B`) in a DER-encoded
/// X.501 `Name` whose value is a ten-character `[A-Z0-9]` `PrintableString`
/// (`0x13`) or `UTF8String` (`0x0C`) — an Apple team id. A pattern scan
/// rather than a full ASN.1 parse: the subject is a flat sequence of
/// attribute pairs, and an `OU` such as the WWDR issuer's `G3` fails the
/// ten-character rule rather than being mistaken for a team.
fn team_from_subject_der(der: &[u8]) -> Option<String> {
    const OU_OID: [u8; 5] = [0x06, 0x03, 0x55, 0x04, 0x0B];
    const TEAM_LEN: usize = 10;
    der.windows(OU_OID.len() + 2 + TEAM_LEN).find_map(|window| {
        let (oid, rest) = window.split_at(OU_OID.len());
        if oid != OU_OID || !(rest[0] == 0x13 || rest[0] == 0x0C) || rest[1] as usize != TEAM_LEN {
            return None;
        }
        let value = &rest[2..];
        value
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            .then(|| String::from_utf8_lossy(value).into_owned())
    })
}

/// Returns the first `(XXXXXXXXXX)` substring whose 10 characters are all
/// `[A-Z0-9]` — the parenthesized id in a certificate name. The team id for a
/// distribution certificate; the *developer's* id for an organization-team
/// development certificate, which is why it is only the fallback.
fn extract_team(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    (0..bytes.len()).find_map(|open| {
        if bytes[open] != b'(' {
            return None;
        }
        let start = open + 1;
        let end = start + 10;
        (end < bytes.len()
            && bytes[end] == b')'
            && bytes[start..end]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
        .then(|| line[start..end].to_string())
    })
}

fn team_from_toml(project_dir: &Path) -> Result<Option<String>> {
    let path = project_dir.join("frust.toml");
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading `{}`", path.display()))?;
    let parsed: TeamToml =
        toml::from_str(&raw).with_context(|| format!("parsing `{}`", path.display()))?;
    Ok(parsed.ios.and_then(|i| i.team).and_then(non_empty))
}

fn non_empty(s: String) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    const FIND_IDENTITY: &str = "find-identity -v -p codesigning";

    fn identities(stdout: &str) -> FakeProcessRunner {
        FakeProcessRunner::new().with(
            format!("security {FIND_IDENTITY}"),
            Output {
                success: true,
                stdout: stdout.to_string(),
                stderr: String::new(),
            },
        )
    }

    fn temp_project(tag: &str, toml: Option<&str>) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-ios-team-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        if let Some(contents) = toml {
            fs::write(dir.join("frust.toml"), contents).unwrap();
        }
        dir
    }

    /// A `security find-certificate -c <label>` attribute dump whose subject
    /// is `UID=<uid>, CN=<cn>, OU=<ou>, O=Example` — the DER hand-assembled
    /// the way Apple's certificates lay it out, one `SET { SEQUENCE { OID,
    /// value } }` per attribute.
    fn certificate_dump(cn: &str, ou: &str) -> String {
        fn attr(oid: &[u8], tag: u8, value: &str) -> Vec<u8> {
            let mut seq = vec![0x06, oid.len() as u8];
            seq.extend_from_slice(oid);
            seq.push(tag);
            seq.push(value.len() as u8);
            seq.extend_from_slice(value.as_bytes());
            let mut set = vec![0x31, (seq.len() + 2) as u8, 0x30, seq.len() as u8];
            set.extend(seq);
            set
        }
        let mut name = Vec::new();
        name.extend(attr(
            &[0x09, 0x92, 0x26, 0x89, 0x93, 0xF2, 0x2C, 0x64, 0x01, 0x01],
            0x0C,
            "UID0000001",
        ));
        name.extend(attr(&[0x55, 0x04, 0x03], 0x0C, cn));
        name.extend(attr(&[0x55, 0x04, 0x0B], 0x13, ou));
        name.extend(attr(&[0x55, 0x04, 0x0A], 0x0C, "Example"));
        let hex: String = name.iter().map(|b| format!("{b:02X}")).collect();
        format!(
            "keychain: \"login.keychain-db\"\n    \"labl\"<blob>=\"{cn}\"\n    \"subj\"<blob>=0x{hex}  \"0\\\"\n"
        )
    }

    fn with_certificate(runner: FakeProcessRunner, label: &str, ou: &str) -> FakeProcessRunner {
        runner.with(
            format!("security find-certificate -c {label}"),
            Output {
                success: true,
                stdout: certificate_dump(label, ou),
                stderr: String::new(),
            },
        )
    }

    #[test]
    fn detect_reads_the_team_from_the_certificate_ou_not_the_name() {
        // An organization-team development certificate: the name carries the
        // developer's id, the `OU` the team — the a3-03 gate's exact shape.
        let label = "Apple Development: Ada (DEVELOPER1)";
        let runner = with_certificate(
            identities(&format!(
                "  1) HASH1 \"{label}\"\n     1 valid identities found"
            )),
            label,
            "TEAMID1234",
        );
        assert_eq!(detect(&runner).unwrap(), vec!["TEAMID1234"]);
    }

    #[test]
    fn detect_falls_back_to_the_name_when_the_certificate_is_unreadable() {
        // No `find-certificate` response at all: the fake runner errors, and
        // the name's parenthesized id stands in.
        let one = r#"  1) ABCDEF0123 "Apple Development: Ada (TEAMID1234)"
     1 valid identities found"#;
        assert_eq!(detect(&identities(one)).unwrap(), vec!["TEAMID1234"]);
        assert_eq!(detect(&identities("")).unwrap(), Vec::<String>::new());

        let several = r#"  1) HASH1 "Apple Development: Ada (TEAMID1234)"
  2) HASH2 "Apple Distribution: Corp (OTHERTEAM9)"
  3) HASH3 "Apple Development: Bob (TEAMID1234)"
     3 valid identities found"#;
        // TEAMID1234 deduped; order preserved.
        assert_eq!(
            detect(&identities(several)).unwrap(),
            vec!["TEAMID1234", "OTHERTEAM9"]
        );
    }

    #[test]
    fn detect_dedupes_across_certificates_that_share_a_team() {
        let ada = "Apple Development: Ada (DEVELOPER1)";
        let corp = "Apple Distribution: Corp (TEAMID1234)";
        let listing =
            format!("  1) HASH1 \"{ada}\"\n  2) HASH2 \"{corp}\"\n     2 valid identities found");
        let runner = with_certificate(
            with_certificate(identities(&listing), ada, "TEAMID1234"),
            corp,
            "TEAMID1234",
        );
        assert_eq!(detect(&runner).unwrap(), vec!["TEAMID1234"]);
    }

    #[test]
    fn subject_team_ignores_short_or_lowercase_ous_and_malformed_blobs() {
        // The WWDR issuer's `OU=G3` shape, and a lowercase value, are not teams.
        assert_eq!(subject_team(&certificate_dump("Ada", "G3")), None);
        assert_eq!(subject_team(&certificate_dump("Ada", "teamid1234")), None);
        assert_eq!(subject_team("    \"subj\"<blob>=0xZZ\n"), None);
        assert_eq!(subject_team("no subject line at all"), None);
        assert_eq!(
            subject_team(&certificate_dump("Ada (DEVELOPER1)", "TEAMID1234")),
            Some("TEAMID1234".to_string())
        );
    }

    #[test]
    fn identity_label_takes_the_quoted_certificate_name() {
        assert_eq!(
            identity_label("  1) HASH \"Apple Development: Ada (TEAMID1234)\""),
            Some("Apple Development: Ada (TEAMID1234)")
        );
        assert_eq!(identity_label("     1 valid identities found"), None);
        assert_eq!(identity_label("  1) HASH \"\""), None);
    }

    #[test]
    fn extract_team_skips_malformed_lines() {
        let mixed = r#"  garbage without parens
  1) HASH "Apple Development: Ada (SHORT)"
  2) HASH2 "Apple Development: Bob (TEAMID1234)"
  (nope)"#;
        // "(SHORT)" is not 10 chars; "(nope)" is lowercase/too short.
        assert_eq!(detect(&identities(mixed)).unwrap(), vec!["TEAMID1234"]);
    }

    #[test]
    fn resolve_prefers_env_over_toml_and_detect() {
        let dir = temp_project("env-wins", Some("[ios]\nteam = \"TOMLTEAM01\"\n"));
        let env = FakeEnv::new().set(TEAM_ENV, "ENVTEAM0001");
        let runner = identities(r#"  1) H "Apple Development: X (DETECT9999)""#);
        let choice = resolve(&env, &runner, &dir).unwrap();
        assert_eq!(choice.team, "ENVTEAM0001");
        assert_eq!(choice.note, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_prefers_toml_over_detect() {
        let dir = temp_project("toml-wins", Some("[ios]\nteam = \"TOMLTEAM01\"\n"));
        let env = FakeEnv::new();
        let runner = identities(r#"  1) H "Apple Development: X (DETECT9999)""#);
        let choice = resolve(&env, &runner, &dir).unwrap();
        assert_eq!(choice.team, "TOMLTEAM01");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_auto_detects_single_team() {
        let dir = temp_project("detect-one", Some("[app]\nname = \"a\"\norg = \"b\"\n"));
        let env = FakeEnv::new();
        let runner = identities(r#"  1) H "Apple Development: X (DETECT9999)""#);
        let choice = resolve(&env, &runner, &dir).unwrap();
        assert_eq!(choice.team, "DETECT9999");
        assert_eq!(choice.note, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_picks_first_of_several_with_override_note() {
        let dir = temp_project("detect-many", None);
        let env = FakeEnv::new();
        let runner = identities(
            r#"  1) H "Apple Development: X (FIRSTTEAM1)"
  2) H2 "Apple Distribution: Y (SECONDTEA2)""#,
        );
        let choice = resolve(&env, &runner, &dir).unwrap();
        assert_eq!(choice.team, "FIRSTTEAM1");
        let note = choice.note.unwrap();
        assert!(note.contains("FIRSTTEAM1"), "{note}");
        assert!(note.contains("SECONDTEA2"), "{note}");
        assert!(note.contains("FRUST_IOS_TEAM"), "{note}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_errors_actionably_when_no_identity() {
        let dir = temp_project("detect-none", None);
        let env = FakeEnv::new();
        let runner = identities("     0 valid identities found");
        let err = resolve(&env, &runner, &dir).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("no codesigning identity"), "{message}");
        assert!(message.contains("--no-codesign"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_ignores_empty_env_and_toml_values() {
        let dir = temp_project("empty-values", Some("[ios]\nteam = \"  \"\n"));
        let env = FakeEnv::new().set(TEAM_ENV, "   ");
        let runner = identities(r#"  1) H "Apple Development: X (DETECT9999)""#);
        let choice = resolve(&env, &runner, &dir).unwrap();
        assert_eq!(choice.team, "DETECT9999");
        let _ = fs::remove_dir_all(&dir);
    }
}
