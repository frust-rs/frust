//! Pure phase-extraction over streamed session output lines: turns a raw
//! line into a displayable [`PhaseLabel`] (or `None`) for the transient
//! build/install/launch status line (workbook §B10). Everything here is
//! plain data + pure functions — no `AppState`, no `SessionState` machine, no
//! I/O — so it's fixture-tested against real captured toolchain output
//! without a TTY, and [`super::supervisor`] is the only caller that threads
//! it into the session event flow.
//!
//! frust has no daemon handing back structured progress events (unlike
//! fdemon's `app.progress`, a pattern source not a copy source — see
//! `docs/TUI_CODE_STANDARDS.md`); every label here is inferred from what
//! already streams: cargo's own `Compiling <crate> v<ver>` status lines, the
//! drive pipelines' `[gradle]`/`[xcodebuild]`-prefixed tool output
//! (`android_run::gradle`/`ios_run::xcodebuild`), and the drive pipelines'
//! own bare `Installing on …`/`Installed …`/`Launching …` phase lines (the
//! same markers [`super::session::infer_state`] reads for its state
//! machine).
//!
//! **Conservative by design:** [`phase_from_output_line`] returns `None` for
//! anything it doesn't recognize rather than guessing — an unmatched line
//! never fabricates a label, and the caller ([`super::supervisor::feed_lines`])
//! only ever *sets* a label from a positive match. Clearing a stale label
//! once the session's state genuinely advances past `Building`/`Installing`
//! is the caller's job (`crate::engine::update::on_session_event`), not
//! this module's — this module has no concept of "stale", only "this line
//! matched" or "it didn't".

/// A parsed, displayable build/install/launch phase label — the text the
/// transient status line shimmers (workbook §B10). Carries only what it
/// takes to render; the raw source line is not retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseLabel {
    /// A cargo `Compiling <crate> v<ver>` status line. `progress` is a
    /// trailing `(<n>/<m>)` count when the line carries one — plain `cargo
    /// build`/`cargo run` output doesn't emit this by default, so it's
    /// usually `None`; kept as a field (rather than parsed out and
    /// discarded) so a progress-carrying line just works without a shape
    /// change here.
    Compiling {
        crate_name: String,
        progress: Option<(u32, u32)>,
    },
    /// A Gradle task path from a `> Task :app:assembleDebug`-shaped line
    /// (`android_run::gradle`'s `[gradle] ` prefix already stripped).
    GradleTask(String),
    /// An xcodebuild `CompileSwift`/`CompileC` step, holding just the source
    /// file's basename (`ios_run::xcodebuild`'s `[xcodebuild] ` prefix
    /// already stripped).
    XcodebuildCompiling(String),
    /// xcodebuild's link step (a `Ld …` line).
    XcodebuildLinking,
    /// The built artifact is being installed onto the target (the drive
    /// pipelines' own `Installing on …`/`Installed …` marker lines).
    Installing,
    /// The installed app is being launched on the target (the drive
    /// pipelines' own `Launching …` marker line). Short-lived in practice —
    /// the very next line is usually the one that also advances
    /// `SessionState` to `Running`, clearing this label almost immediately.
    Launching,
}

impl PhaseLabel {
    /// The shimmered status-line text (workbook §B10's phase-label column).
    pub fn text(&self) -> String {
        match self {
            PhaseLabel::Compiling {
                crate_name,
                progress: Some((n, m)),
            } => format!("Compiling {crate_name} ({n}/{m})"),
            PhaseLabel::Compiling {
                crate_name,
                progress: None,
            } => format!("Compiling {crate_name}"),
            PhaseLabel::GradleTask(path) => format!("> Task {path}"),
            PhaseLabel::XcodebuildCompiling(file) => format!("Compiling {file}"),
            PhaseLabel::XcodebuildLinking => "Linking".to_string(),
            PhaseLabel::Installing => "Installing\u{2026}".to_string(),
            PhaseLabel::Launching => "Launching\u{2026}".to_string(),
        }
    }
}

/// Extract a phase label from one streamed output line, if it names one this
/// module recognizes. Checks the drive pipelines' own `[gradle]`/
/// `[xcodebuild]` prefixes first (each toolchain's line shape is otherwise
/// ambiguous with the others), then falls back to bare cargo/pipeline text.
/// Returns `None` for anything unrecognized — see the module docs'
/// conservative-extractor contract.
pub fn phase_from_output_line(line: &str) -> Option<PhaseLabel> {
    if let Some(rest) = line.strip_prefix("[gradle] ") {
        return gradle_phase(rest);
    }
    if let Some(rest) = line.strip_prefix("[xcodebuild] ") {
        return xcodebuild_phase(rest);
    }
    bare_phase(line)
}

/// Cargo's own `Compiling …` status line, plus the drive pipelines' bare
/// `Installing …`/`Installed …`/`Launching …` phase markers — the same three
/// prefixes [`super::session::phase_from_line`] reads for the
/// `SessionState` machine, read here instead for their display text.
fn bare_phase(line: &str) -> Option<PhaseLabel> {
    // Cargo indents its status lines ("   Compiling"); the drive pipelines'
    // own markers don't — trimming handles both (mirrors
    // `super::session::phase_from_line`).
    let l = line.trim_start();
    if let Some(rest) = l.strip_prefix("Compiling ") {
        return compiling_label(rest);
    }
    if l.starts_with("Installing") || l.starts_with("Installed") {
        return Some(PhaseLabel::Installing);
    }
    if l.starts_with("Launching") {
        return Some(PhaseLabel::Launching);
    }
    None
}

/// Parse cargo's `Compiling <crate> v<version> […]` status line into a
/// [`PhaseLabel::Compiling`]: the crate name is the first whitespace token,
/// the version and any parenthesized path suffix are dropped. `progress`
/// captures a trailing `(<n>/<m>)` count when present ([`trailing_progress`]).
fn compiling_label(rest: &str) -> Option<PhaseLabel> {
    let crate_name = rest.split_whitespace().next()?.to_string();
    if crate_name.is_empty() {
        return None;
    }
    Some(PhaseLabel::Compiling {
        crate_name,
        progress: trailing_progress(rest),
    })
}

/// A trailing `(<n>/<m>)` count at the end of `line` (surrounding whitespace
/// tolerated), or `None` if the line doesn't end in that shape.
fn trailing_progress(line: &str) -> Option<(u32, u32)> {
    let l = line.trim_end().strip_suffix(')')?;
    let (_, paren) = l.rsplit_once('(')?;
    let (n, m) = paren.split_once('/')?;
    let n: u32 = n.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    Some((n, m))
}

/// A Gradle task line, already stripped of its `[gradle] ` prefix —
/// `> Task :app:assembleDebug` → `GradleTask(":app:assembleDebug")`. Every
/// other Gradle output line (compiler warnings, `BUILD SUCCESSFUL`, …) is
/// unrecognized.
fn gradle_phase(line: &str) -> Option<PhaseLabel> {
    let path = line.trim_start().strip_prefix("> Task ")?.trim();
    if path.is_empty() {
        return None;
    }
    Some(PhaseLabel::GradleTask(path.to_string()))
}

/// An xcodebuild output line, already stripped of its `[xcodebuild] `
/// prefix. Recognizes the per-file `CompileSwift`/`CompileC` compile steps
/// and the `Ld …` link step — xcodebuild's other step lines (`CodeSign`,
/// `ProcessInfoPlistFile`, the `=== BUILD TARGET … ===` banners, …) are
/// unrecognized.
///
/// **Community-approximate line shape**: xcodebuild's non-`-quiet` verbose
/// output format has no published spec; `CompileSwift normal <arch>
/// <path> …`/`CompileC <obj> <path> normal <arch> …` and `Ld <path> normal
/// …` are where community tooling (xcbeautify and similar) converges.
fn xcodebuild_phase(line: &str) -> Option<PhaseLabel> {
    let l = line.trim_start();
    if l.starts_with("Ld ") {
        return Some(PhaseLabel::XcodebuildLinking);
    }
    // The umbrella `CompileSwiftSources` step names no single file; the
    // per-file `CompileSwift` lines that follow it do.
    if l.starts_with("CompileSwiftSources") {
        return None;
    }
    if l.starts_with("CompileSwift") || l.starts_with("CompileC") {
        let file = l
            .split_whitespace()
            .rfind(|tok| tok.contains('/'))
            .and_then(|tok| tok.rsplit('/').next())
            .unwrap_or("source")
            .to_string();
        return Some(PhaseLabel::XcodebuildCompiling(file));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Cargo ────────────────────────────────────────────────────────────

    #[test]
    fn cargo_compiling_line_yields_crate_name_with_no_progress() {
        assert_eq!(
            phase_from_output_line("   Compiling frust-core v0.3.1 (/repo/crates/frust-core)"),
            Some(PhaseLabel::Compiling {
                crate_name: "frust-core".to_string(),
                progress: None,
            })
        );
    }

    #[test]
    fn cargo_compiling_line_with_trailing_progress_count() {
        assert_eq!(
            phase_from_output_line("   Compiling frust-core v0.3.1 (41/210)"),
            Some(PhaseLabel::Compiling {
                crate_name: "frust-core".to_string(),
                progress: Some((41, 210)),
            })
        );
        assert_eq!(
            PhaseLabel::Compiling {
                crate_name: "frust-core".to_string(),
                progress: Some((41, 210)),
            }
            .text(),
            "Compiling frust-core (41/210)"
        );
    }

    #[test]
    fn cargo_finished_and_running_lines_are_unrecognized() {
        // These are `super::session::infer_state`'s own state markers, not
        // phase text — labeling "Running `target/debug/app`" as "Launching"
        // would be actively misleading (the desktop preview has no separate
        // install/launch step).
        assert_eq!(
            phase_from_output_line("    Finished dev [unoptimized + debuginfo] target(s) in 1.2s"),
            None
        );
        assert_eq!(
            phase_from_output_line("     Running `target/debug/app`"),
            None
        );
    }

    #[test]
    fn plain_unrelated_output_is_unrecognized() {
        assert_eq!(phase_from_output_line("warning: unused import"), None);
        assert_eq!(phase_from_output_line("app: booting up"), None);
        assert_eq!(phase_from_output_line(""), None);
    }

    // ── Gradle ───────────────────────────────────────────────────────────

    #[test]
    fn gradle_task_line_yields_the_task_path() {
        assert_eq!(
            phase_from_output_line("[gradle] > Task :app:assembleDebug"),
            Some(PhaseLabel::GradleTask(":app:assembleDebug".to_string()))
        );
        assert_eq!(
            PhaseLabel::GradleTask(":app:assembleDebug".to_string()).text(),
            "> Task :app:assembleDebug"
        );
    }

    #[test]
    fn gradle_non_task_lines_are_unrecognized() {
        assert_eq!(
            phase_from_output_line("[gradle] BUILD SUCCESSFUL in 12s"),
            None
        );
        assert_eq!(
            phase_from_output_line("[gradle] e: MainActivity.kt: (42, 9): Unresolved reference"),
            None
        );
    }

    // ── xcodebuild ───────────────────────────────────────────────────────

    #[test]
    fn xcodebuild_compile_swift_line_yields_the_file_basename() {
        assert_eq!(
            phase_from_output_line(
                "[xcodebuild] CompileSwift normal arm64 /repo/ios/Runner/AppDelegate.swift (in target 'Runner' from project 'Runner')"
            ),
            Some(PhaseLabel::XcodebuildCompiling(
                "AppDelegate.swift".to_string()
            ))
        );
    }

    #[test]
    fn xcodebuild_compile_c_line_picks_the_source_not_the_object_path() {
        assert_eq!(
            phase_from_output_line(
                "[xcodebuild] CompileC /repo/build/obj/Runner-main.o /repo/ios/Runner/main.m normal arm64 objective-c com.apple.compilers.llvm.clang.1_0.compiler"
            ),
            Some(PhaseLabel::XcodebuildCompiling("main.m".to_string()))
        );
    }

    #[test]
    fn xcodebuild_link_line_yields_linking() {
        assert_eq!(
            phase_from_output_line("[xcodebuild] Ld /repo/build/ios/Runner.app/Runner normal"),
            Some(PhaseLabel::XcodebuildLinking)
        );
        assert_eq!(PhaseLabel::XcodebuildLinking.text(), "Linking");
    }

    #[test]
    fn xcodebuild_umbrella_and_other_steps_are_unrecognized() {
        assert_eq!(
            phase_from_output_line(
                "[xcodebuild] CompileSwiftSources normal arm64 com.apple.xcode.tools.swift.compiler"
            ),
            None
        );
        assert_eq!(
            phase_from_output_line("[xcodebuild] === BUILD TARGET Runner OF PROJECT Runner ==="),
            None
        );
        assert_eq!(
            phase_from_output_line("[xcodebuild] ** BUILD SUCCEEDED **"),
            None
        );
    }

    // ── Install / launch (the drive pipelines' own bare marker lines) ──────

    #[test]
    fn install_marker_lines_yield_installing() {
        assert_eq!(
            phase_from_output_line("Installing on Pixel 7…"),
            Some(PhaseLabel::Installing)
        );
        assert_eq!(
            phase_from_output_line("Installed in 2.3s."),
            Some(PhaseLabel::Installing)
        );
        assert_eq!(PhaseLabel::Installing.text(), "Installing\u{2026}");
    }

    #[test]
    fn launch_marker_line_yields_launching() {
        assert_eq!(
            phase_from_output_line("Launching it.f0x.huddle…"),
            Some(PhaseLabel::Launching)
        );
        assert_eq!(PhaseLabel::Launching.text(), "Launching\u{2026}");
    }

    #[test]
    fn streaming_logs_marker_is_unrecognized() {
        // `infer_state` reads this the same as `Launching`/`Running` (all
        // three mean the app has reached `Running`), but it names no
        // meaningful phase text of its own — matches the "unrecognized ⇒
        // None" fallback the transient status line relies on to fall back to
        // a plain "Building…"/no label rather than a fabricated one.
        assert_eq!(phase_from_output_line("Streaming logs (pid 4242)"), None);
    }
}
