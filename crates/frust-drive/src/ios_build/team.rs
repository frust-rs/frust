//! `DEVELOPMENT_TEAM` resolution for signed iOS builds (spec §12.6, §16).
//!
//! Precedence: `FRUST_IOS_TEAM` env → `[ios] team` in `frust.toml` →
//! auto-detect from `security find-identity -v -p codesigning`. Zero detected
//! teams while signing is required is an actionable error; multiple picks the
//! first and prints how to override.

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

/// Runs `security find-identity -v -p codesigning` and extracts the distinct
/// team ids from its identity lines.
pub fn detect(runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    let out = runner
        .run("security", &["find-identity", "-v", "-p", "codesigning"])
        .context("running `security find-identity`")?;
    Ok(parse_identities(&out.stdout))
}

/// Parses `security find-identity` output, taking the first
/// `([A-Z0-9]{10})` token on each identity line as its team id and deduping
/// across identities (preserving first-seen order).
fn parse_identities(output: &str) -> Vec<String> {
    let mut teams: Vec<String> = Vec::new();
    for line in output.lines() {
        if let Some(team) = extract_team(line)
            && !teams.contains(&team)
        {
            teams.push(team);
        }
    }
    teams
}

/// Returns the first `(XXXXXXXXXX)` substring whose 10 characters are all
/// `[A-Z0-9]` — the certificate CN's parenthesized team id.
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

    #[test]
    fn parse_identities_handles_zero_one_and_several() {
        assert_eq!(parse_identities(""), Vec::<String>::new());

        let one = r#"  1) ABCDEF0123 "Apple Development: Ada (TEAMID1234)"
     1 valid identities found"#;
        assert_eq!(parse_identities(one), vec!["TEAMID1234"]);

        let several = r#"  1) HASH1 "Apple Development: Ada (TEAMID1234)"
  2) HASH2 "Apple Distribution: Corp (OTHERTEAM9)"
  3) HASH3 "Apple Development: Bob (TEAMID1234)"
     3 valid identities found"#;
        // TEAMID1234 deduped; order preserved.
        assert_eq!(parse_identities(several), vec!["TEAMID1234", "OTHERTEAM9"]);
    }

    #[test]
    fn parse_identities_skips_malformed_lines() {
        let mixed = r#"  garbage without parens
  1) HASH "Apple Development: Ada (SHORT)"
  2) HASH2 "Apple Development: Bob (TEAMID1234)"
  (nope)"#;
        // "(SHORT)" is not 10 chars; "(nope)" is lowercase/too short.
        assert_eq!(parse_identities(mixed), vec!["TEAMID1234"]);
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
