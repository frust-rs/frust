//! Session-family tools: what can be run, what is running, starting and
//! stopping it, and reading back its output.
//!
//! Every one of these is a thin mapping over
//! [`crate::backend::SessionBackend`] — the backend owns the supervision,
//! this module owns the wire shape (the backend's types are plain Rust and
//! carry no serde/schema derives, on purpose: several of them are
//! `frust-drive`'s).
//!
//! The three **blocking** backend calls — `list_devices`, `stop_app`,
//! `restart_app` — are issued from [`tokio::task::spawn_blocking`] here, since
//! each of them waits on a device that may never answer.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use frust_devtools_protocol::Capability;
use frust_drive::build_info::BuildMode;
use frust_drive::devices::{Device, Kind, Platform};
use rmcp::handler::server::wrapper::Json;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::{ToolError, ToolResult, resolve_session, session_ref, state_name};
use crate::backend::SharedBackend;
use crate::engine::{RunTarget, SessionSnapshot, SessionState};

/// Default number of log lines `app_logs` returns.
const DEFAULT_LOG_LIMIT: usize = 100;

/// Hard cap on `app_logs`'s `limit` — a whole 10k-line ring in one tool
/// result is a payload no agent reads and every transport pays for.
const MAX_LOG_LIMIT: usize = 2_000;

// ── DTOs ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct DeviceDto {
    /// The id `run_app`'s `target` takes (an `adb` serial, or a Simulator
    /// udid).
    pub id: String,
    pub name: String,
    /// `android` or `ios`.
    pub platform: &'static str,
    /// `physical`, `emulator`, or `simulator`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_state: Option<String>,
    /// Whether `run_app` can target this device today (a physical iOS device
    /// cannot be supervised by this server — see the field's message).
    pub runnable: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ListDevicesResult {
    /// `desktop` is always a valid `run_app` target and is not listed here.
    pub devices: Vec<DeviceDto>,
    /// Non-fatal discovery notes (a missing SDK, an unauthorized device).
    pub notes: Vec<String>,
}

/// One session, as the tool layer reports it.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SessionDto {
    pub id: u64,
    /// `desktop`, `android:<serial>`, or `ios-sim:<udid>`.
    pub target: String,
    pub mode: &'static str,
    pub project_root: String,
    /// `launching`, `running`, `devtools_connected`, `exited`, or `failed`.
    pub state: &'static str,
    /// For `exited`: whether the process reported success. Never an exit
    /// code — the process seam does not expose one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_success: Option<bool>,
    /// For `failed`: the build/install/launch error chain, verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub android_package: Option<String>,
    /// The bundle id an iOS Simulator session launched — the app teardown
    /// terminates, and the one an agent would pass to `simctl` itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ios_bundle_id: Option<String>,
    /// The loopback port this server reaches the app's devtools service on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devtools_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devtools_app_name: Option<String>,
    /// What the app declared at handshake (`widget_tree`, `frame_stats`,
    /// `input`, `metrics`, `screenshot`) — the tools that will work.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub devtools_capabilities: Vec<String>,
    /// Why devtools is unavailable, as the app or this server reported it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devtools_error: Option<String>,
    pub log_lines: usize,
    pub dropped_log_lines: u64,
    pub frames: usize,
    pub dropped_frames: u64,
    /// Whether system metrics are being sampled. Android only — no other
    /// target exposes the app's pid to this server.
    pub metrics_sampling: bool,
    pub started_unix_ms: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ListSessionsResult {
    pub sessions: Vec<SessionDto>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct RunAppResult {
    pub session: SessionDto,
    /// What to do next — `run_app` returns before the app is up.
    pub note: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct StopAppResult {
    pub session: SessionDto,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct RestartAppResult {
    /// The stopped session, keeping its own final state.
    pub previous: SessionDto,
    /// The freshly launched replacement.
    pub session: SessionDto,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct AppLogsResult {
    pub session_id: u64,
    /// The most recent matching lines, oldest first.
    pub lines: Vec<String>,
    /// How many lines matched the filters in total (`lines` is the tail of
    /// this).
    pub matched: usize,
    /// How many lines the session currently retains.
    pub retained: usize,
    /// How many were evicted for ring capacity over the session's life.
    pub dropped: u64,
}

// ── Arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RunAppArgs {
    /// `desktop` for a host preview, or a device id/name from list_devices
    /// (a unique case-insensitive substring is enough).
    pub target: String,
    /// `debug` (default) or `profile`. Both build with the devtools service
    /// enabled; `release` is deliberately not offered, since a release build
    /// compiles the service out and no other tool here would work.
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SessionArgs {
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AppLogsArgs {
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
    /// How many of the most recent matching lines to return (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Case-insensitive substring the line must contain (not a regex).
    #[serde(default)]
    pub pattern: Option<String>,
    /// `error`, `warn`, `info`, `debug`, or `trace`. A textual match — app
    /// output is plain lines, so this looks for the level word or an Android
    /// logcat priority tag (`E/`, `W/`, …), not a structured field.
    #[serde(default)]
    pub level: Option<String>,
}

// ── Tools ───────────────────────────────────────────────────────────────────

pub(crate) async fn list_devices(backend: &SharedBackend) -> ToolResult<ListDevicesResult> {
    let (devices, notes) = discover_devices(backend).await;
    Ok(Json(ListDevicesResult {
        devices: devices.iter().map(device_dto).collect(),
        notes,
    }))
}

pub(crate) fn list_sessions(backend: &SharedBackend) -> ToolResult<ListSessionsResult> {
    Ok(Json(ListSessionsResult {
        sessions: backend.sessions().iter().map(session_dto).collect(),
    }))
}

pub(crate) async fn run_app(backend: &SharedBackend, args: RunAppArgs) -> ToolResult<RunAppResult> {
    let mode = match parse_mode(args.mode.as_deref()) {
        Ok(mode) => mode,
        Err(err) => return Err(Json(err)),
    };
    let target = match resolve_target(backend, &args.target).await {
        Ok(target) => target,
        Err(err) => return Err(Json(err)),
    };

    let id = backend.run_app(target, mode);
    let Some(snapshot) = backend.session(id) else {
        return Err(Json(ToolError::new(
            "the session vanished immediately after launch — this is a bug in frust-mcp",
        )));
    };
    Ok(Json(RunAppResult {
        session: session_dto(&snapshot),
        note: format!(
            "launch started for session {id}; it builds, installs, and connects in the \
             background. Poll list_sessions until state is devtools_connected (or failed), \
             and read app_logs for progress."
        ),
    }))
}

pub(crate) async fn stop_app(
    backend: &SharedBackend,
    args: SessionArgs,
) -> ToolResult<StopAppResult> {
    let snapshot = match resolve_session(backend, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let id = snapshot.id;
    let stopping = Arc::clone(backend);
    let stopped = tokio::task::spawn_blocking(move || stopping.stop_app(id)).await;
    match stopped {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(Json(ToolError::new(format!(
                "stopping session {id} failed: {err:#}"
            ))));
        }
        Err(err) => {
            return Err(Json(ToolError::new(format!(
                "the stop task for session {id} failed: {err}"
            ))));
        }
    }
    let stopped = backend.session(id).unwrap_or(snapshot);
    Ok(Json(StopAppResult {
        session: session_dto(&stopped),
    }))
}

pub(crate) async fn restart_app(
    backend: &SharedBackend,
    args: SessionArgs,
) -> ToolResult<RestartAppResult> {
    let snapshot = match resolve_session(backend, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let id = snapshot.id;
    let restarting = Arc::clone(backend);
    let new_id = match tokio::task::spawn_blocking(move || restarting.restart_app(id)).await {
        Ok(Ok(new_id)) => new_id,
        Ok(Err(err)) => {
            return Err(Json(ToolError::new(format!(
                "restarting session {id} failed: {err:#}"
            ))));
        }
        Err(err) => {
            return Err(Json(ToolError::new(format!(
                "the restart task for session {id} failed: {err}"
            ))));
        }
    };
    let previous = backend.session(id).unwrap_or(snapshot);
    let Some(session) = backend.session(new_id) else {
        return Err(Json(ToolError::new(
            "the restarted session vanished immediately after launch — this is a bug in frust-mcp",
        )));
    };
    Ok(Json(RestartAppResult {
        previous: session_dto(&previous),
        session: session_dto(&session),
    }))
}

pub(crate) fn app_logs(backend: &SharedBackend, args: AppLogsArgs) -> ToolResult<AppLogsResult> {
    let snapshot = match resolve_session(backend, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let Some(lines) = backend.logs(snapshot.id, None) else {
        return Err(Json(
            ToolError::new(format!("no such session: {}", snapshot.id))
                .with_sessions(vec![session_ref(&snapshot)]),
        ));
    };
    let limit = args.limit.unwrap_or(DEFAULT_LOG_LIMIT).min(MAX_LOG_LIMIT);
    let (tail, matched) = filter_logs(
        &lines,
        args.pattern.as_deref(),
        args.level.as_deref(),
        limit,
    );
    Ok(Json(AppLogsResult {
        session_id: snapshot.id.0,
        lines: tail,
        matched,
        retained: snapshot.log_lines,
        dropped: snapshot.dropped_log_lines,
    }))
}

// ── Mapping ─────────────────────────────────────────────────────────────────

/// Discovery, off the runtime: it shells out to `adb`/`xcrun`, either of
/// which an unresponsive device can stall without bound. A task that fails
/// outright is reported as a discovery *note* rather than a tool error —
/// `desktop` is still a valid target with no device list at all.
async fn discover_devices(backend: &SharedBackend) -> (Vec<Device>, Vec<String>) {
    let backend = Arc::clone(backend);
    tokio::task::spawn_blocking(move || backend.list_devices())
        .await
        .unwrap_or_else(|err| (Vec::new(), vec![format!("device discovery failed: {err}")]))
}

/// Filters `lines` by an optional substring and level, then keeps the most
/// recent `limit`. Returns the tail plus how many matched in total, so a
/// caller can tell "these are all of them" from "these are the last 100".
pub(crate) fn filter_logs(
    lines: &[String],
    pattern: Option<&str>,
    level: Option<&str>,
    limit: usize,
) -> (Vec<String>, usize) {
    let needle = pattern.map(str::to_lowercase);
    let matched: Vec<&String> = lines
        .iter()
        .filter(|line| {
            needle
                .as_deref()
                .is_none_or(|needle| line.to_lowercase().contains(needle))
                && level.is_none_or(|level| line_has_level(line, level))
        })
        .collect();
    let total = matched.len();
    let start = total.saturating_sub(limit);
    (
        matched[start..].iter().map(|s| (*s).clone()).collect(),
        total,
    )
}

/// Whether `line` looks like it carries `level`.
///
/// App output is unstructured text — there is no level field on the wire —
/// so this matches the level word (`ERROR`, `warn`, …) or an Android logcat
/// priority tag (`E/`, `W/`, …, and the `E ` column of the threadtime
/// format). Deliberately a heuristic, and documented as one on the tool's
/// `level` argument rather than dressed up as a parsed field.
fn line_has_level(line: &str, level: &str) -> bool {
    let (word, letter) = match level.to_ascii_lowercase().as_str() {
        "error" => ("error", 'E'),
        "warn" | "warning" => ("warn", 'W'),
        "info" => ("info", 'I'),
        "debug" => ("debug", 'D'),
        "trace" | "verbose" => ("trace", 'V'),
        // An unrecognized level filters on the raw word rather than
        // silently matching everything.
        other => return line.to_lowercase().contains(other),
    };
    if line.to_lowercase().contains(word) {
        return true;
    }
    // logcat priority column: `<letter>/<tag>` (brief) or a lone `<letter>`
    // token (threadtime).
    line.split_whitespace().any(|token| {
        let mut chars = token.chars();
        chars.next() == Some(letter) && matches!(chars.next(), None | Some('/'))
    })
}

fn parse_mode(mode: Option<&str>) -> Result<BuildMode, ToolError> {
    match mode.map(str::to_ascii_lowercase).as_deref() {
        None | Some("debug") => Ok(BuildMode::Debug),
        Some("profile") => Ok(BuildMode::Profile),
        Some("release") => Err(ToolError::new(
            "mode \"release\" is not offered: a release build compiles the devtools service \
             out, so no inspection or input tool would work. Use \"debug\" or \"profile\".",
        )),
        Some(other) => Err(ToolError::new(format!(
            "unknown mode {other:?} — use \"debug\" or \"profile\"."
        ))),
    }
}

/// Maps `target` onto a [`RunTarget`]: the literal `desktop`, or a device
/// matched by exact id first, then by unique case-insensitive substring of
/// its id or name.
async fn resolve_target(backend: &SharedBackend, target: &str) -> Result<RunTarget, ToolError> {
    if target.eq_ignore_ascii_case("desktop") {
        return Ok(RunTarget::Desktop);
    }
    let (devices, notes) = discover_devices(backend).await;
    let exact: Vec<&Device> = devices.iter().filter(|d| d.id == target).collect();
    let matches: Vec<&Device> = if exact.is_empty() {
        let needle = target.to_lowercase();
        devices
            .iter()
            .filter(|d| {
                d.id.to_lowercase().contains(&needle) || d.name.to_lowercase().contains(&needle)
            })
            .collect()
    } else {
        exact
    };

    match matches.as_slice() {
        [device] => run_target_for(device),
        [] => Err(ToolError::new(format!(
            "no device matched {target:?}. Known targets: \"desktop\"{}{}",
            devices
                .iter()
                .map(|d| format!(", {:?} ({})", d.id, d.name))
                .collect::<String>(),
            if notes.is_empty() {
                String::new()
            } else {
                format!(". Discovery notes: {}", notes.join("; "))
            }
        ))),
        many => Err(ToolError::new(format!(
            "{target:?} matched {} devices: {}. Pass a full device id.",
            many.len(),
            many.iter()
                .map(|d| format!("{:?} ({})", d.id, d.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn run_target_for(device: &Device) -> Result<RunTarget, ToolError> {
    match (device.platform, device.kind) {
        (Platform::Android, _) => Ok(RunTarget::Android(device.clone())),
        (Platform::Ios, Kind::Simulator) => Ok(RunTarget::IosSimulator(device.clone())),
        (Platform::Ios, _) => Err(ToolError::new(format!(
            "{:?} is a physical iOS device; this server can only supervise desktop, Android, \
             and iOS Simulator targets. Run it from the frust CLI instead.",
            device.id
        ))),
    }
}

fn device_dto(device: &Device) -> DeviceDto {
    DeviceDto {
        id: device.id.clone(),
        name: device.name.clone(),
        platform: match device.platform {
            Platform::Android => "android",
            Platform::Ios => "ios",
        },
        kind: match device.kind {
            Kind::PhysicalDevice => "physical",
            Kind::Emulator => "emulator",
            Kind::Simulator => "simulator",
        },
        os_version: device.os_version.clone(),
        connection_state: device.connection_state.clone(),
        runnable: run_target_for(device).is_ok(),
    }
}

pub(crate) fn session_dto(snapshot: &SessionSnapshot) -> SessionDto {
    let (exit_success, failure_reason) = match &snapshot.state {
        SessionState::Exited { success } => (Some(*success), None),
        SessionState::Failed { reason } => (None, Some(reason.clone())),
        _ => (None, None),
    };
    SessionDto {
        id: snapshot.id.0,
        target: snapshot.target.label(),
        mode: mode_name(snapshot.mode),
        project_root: snapshot.project_root.display().to_string(),
        state: state_name(&snapshot.state),
        exit_success,
        failure_reason,
        pid: snapshot.pid.clone(),
        android_package: snapshot.android_package.clone(),
        ios_bundle_id: snapshot.ios_bundle_id.clone(),
        devtools_port: snapshot.devtools_port,
        devtools_app_name: snapshot
            .devtools_handshake
            .as_ref()
            .map(|info| info.app_name.clone()),
        devtools_capabilities: snapshot
            .devtools_capabilities()
            .unwrap_or_default()
            .iter()
            .map(|capability| capability_name(*capability).to_string())
            .collect(),
        devtools_error: snapshot.devtools_error.clone(),
        log_lines: snapshot.log_lines,
        dropped_log_lines: snapshot.dropped_log_lines,
        frames: snapshot.frames,
        dropped_frames: snapshot.dropped_frames,
        metrics_sampling: snapshot.metrics_sampling,
        started_unix_ms: snapshot
            .started_at
            .duration_since(UNIX_EPOCH)
            .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0),
    }
}

/// The capability's wire name — the same snake_case the protocol leaf's own
/// serde derive produces, so a reported capability reads identically here
/// and on the wire.
fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::WidgetTree => "widget_tree",
        Capability::FrameStats => "frame_stats",
        Capability::Input => "input",
        Capability::Metrics => "metrics",
        Capability::Screenshot => "screenshot",
        Capability::Unknown => "unknown",
    }
}

fn mode_name(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_limit_keeps_the_most_recent_matches() {
        let all = lines(&["one", "two", "three", "four"]);
        let (tail, matched) = filter_logs(&all, None, None, 2);
        assert_eq!(tail, lines(&["three", "four"]));
        assert_eq!(matched, 4);
    }

    #[test]
    fn a_limit_past_the_end_returns_everything_matched() {
        let all = lines(&["one", "two"]);
        let (tail, matched) = filter_logs(&all, None, None, 100);
        assert_eq!(tail, all);
        assert_eq!(matched, 2);
    }

    #[test]
    fn the_pattern_is_a_case_insensitive_substring() {
        let all = lines(&["Compiling frust", "warning: unused", "Running app"]);
        let (tail, matched) = filter_logs(&all, Some("RUN"), None, 10);
        assert_eq!(tail, lines(&["Running app"]));
        assert_eq!(matched, 1);
    }

    #[test]
    fn the_pattern_and_level_filters_compose() {
        let all = lines(&[
            "E/frust: boom in the widget tree",
            "E/other: boom elsewhere",
            "I/frust: fine",
        ]);
        let (tail, matched) = filter_logs(&all, Some("frust"), Some("error"), 10);
        assert_eq!(tail, lines(&["E/frust: boom in the widget tree"]));
        assert_eq!(matched, 1);
    }

    #[test]
    fn the_level_filter_matches_words_and_logcat_priority_tags() {
        assert!(line_has_level("ERROR: could not bind", "error"));
        assert!(line_has_level("E/frust-devtools: nope", "error"));
        assert!(line_has_level(
            "08-09 12:00:00.000  123  456 W frust: careful",
            "warn"
        ));
        assert!(!line_has_level("I/frust: all good", "error"));
        // A bare `E` inside a word is not a priority column.
        assert!(!line_has_level("Everything is fine", "error"));
    }

    #[test]
    fn an_unknown_level_filters_on_the_raw_word() {
        assert!(line_has_level("FATAL: gone", "fatal"));
        assert!(!line_has_level("all good", "fatal"));
    }

    #[test]
    fn release_mode_is_refused_with_the_reason() {
        let err = parse_mode(Some("release")).expect_err("release is not offered");
        assert!(err.error.contains("devtools"), "unhelpful: {}", err.error);
        assert_eq!(parse_mode(None).expect("default"), BuildMode::Debug);
        assert_eq!(
            parse_mode(Some("PROFILE")).expect("case-insensitive"),
            BuildMode::Profile
        );
        assert!(parse_mode(Some("fast")).is_err());
    }

    #[test]
    fn a_physical_ios_device_is_reported_as_not_runnable() {
        let device = Device {
            id: "00008120-001".to_string(),
            name: "Ed's iPhone".to_string(),
            platform: Platform::Ios,
            kind: Kind::PhysicalDevice,
            os_version: Some("17.5".to_string()),
            connection_state: Some("connected".to_string()),
        };
        assert!(!device_dto(&device).runnable);
        assert!(run_target_for(&device).is_err());
    }
}
