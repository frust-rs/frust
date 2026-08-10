//! Diagnosis-family tools: what the app looks like, how fast it is running,
//! what it is costing, and what is on screen.
//!
//! # Bounded payloads
//!
//! Every reader here caps what it returns and *says* it capped: the widget
//! tree marks each elided subtree with the child count it dropped, and the
//! frame reader reports its sample count and window. A silently truncated
//! payload is worse than a small one — an agent reasons about what it was
//! given as if it were the whole picture.
//!
//! # Never zeros for missing data
//!
//! System metrics are Android-only (the process seam exposes no pid for a
//! desktop or Simulator session — see [`crate::engine`]'s module doc), and an
//! app that has rendered nothing has no frames. Both report *unavailable*,
//! with the reason, rather than a plausible-looking zero.

use frust_devtools_protocol::{Capability, FrameStats, WidgetNode, WidgetTreeDump};
use frust_drive::process::ProcessRunner;
use rmcp::handler::server::wrapper::Json;
use rmcp::model::{CallToolResult, Content};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::session::SessionArgs;
use super::{Rect, ToolError, ToolResult, devtools_call, resolve_session, round2, us_to_ms};
use crate::engine::{SessionEngine, SessionSnapshot};

/// Default depth `widget_tree` descends below each root.
const DEFAULT_TREE_DEPTH: usize = 12;

/// Hard cap on `widget_tree`'s `depth`.
const MAX_TREE_DEPTH: usize = 50;

/// The frame budget a sample is counted as jank past: one 60Hz frame.
///
/// Reported alongside every aggregate as `jank_threshold_ms`, because a
/// 90/120Hz device's real budget is shorter and a caller must be able to see
/// which bar was applied.
const JANK_BUDGET_MS: f64 = 16.7;

/// PNG's file signature — the first bytes of any well-formed PNG (RFC 2083
/// §3.1). Screenshot payloads are checked against it so a base64 blob that
/// is really a shell error message never reaches an agent as an image.
const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

// ── Arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WidgetTreeArgs {
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
    /// How many levels to descend below each root (default 12, max 50).
    /// Deeper subtrees are elided and marked with `children_truncated`.
    #[serde(default)]
    pub depth: Option<usize>,
}

// ── Results ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct WidgetTreeResult {
    pub session_id: u64,
    /// The depth cap actually applied.
    pub depth: usize,
    /// Nodes included in this payload.
    pub node_count: usize,
    /// Whether any subtree was elided — re-query with a larger `depth`.
    pub truncated: bool,
    pub roots: Vec<TreeNode>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct TreeNode {
    pub id: u64,
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_label: Option<String>,
    /// Logical px.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
    /// Set instead of `children` when the depth cap stopped the walk here:
    /// how many direct children were dropped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children_truncated: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct PerformanceResult {
    pub session_id: u64,
    /// Frame samples currently retained for this session.
    pub sample_count: usize,
    /// Samples evicted for ring capacity over the session's life.
    pub dropped_samples: u64,
    /// Why there is nothing to aggregate, when `stats` is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Absent — never zeroed — when no frames have arrived, or when every
    /// frame that has arrived was skipped by the mobile frame gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<FrameAggregate>,
}

#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub(crate) struct FrameAggregate {
    /// Frames per second implied by the mean frame time — not a wall-clock
    /// rate: the wire carries no timestamps, only per-frame durations.
    pub fps_estimate: f64,
    pub frame_ms_mean: f64,
    pub frame_ms_p50: f64,
    pub frame_ms_p95: f64,
    pub frame_ms_p99: f64,
    pub frame_ms_max: f64,
    /// Samples over `jank_threshold_ms`.
    pub jank_frames: usize,
    pub jank_percent: f64,
    pub jank_threshold_ms: f64,
    /// Samples the shell's frame gate marked as skipped.
    pub skipped_frames: usize,
    /// `last.n - first.n + 1`: how many frames the app counted across this
    /// window, including any this server never received.
    pub frame_index_span: u64,
    /// The summed duration of the sampled frames — the time the app spent
    /// *rendering* them, not the wall-clock span they cover.
    pub sampled_span_ms: f64,
    pub phase_means_ms: PhaseMeans,
}

/// Mean time per pipeline phase across the window, in ms. Field names mirror
/// `FrameStats`'s own, so a reading here maps 1:1 onto the app's perf output.
#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub(crate) struct PhaseMeans {
    pub rebuild: f64,
    pub layout: f64,
    pub paint: f64,
    pub encode: f64,
    pub acquire: f64,
    pub submit: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct MetricsResult {
    pub session_id: u64,
    /// Sampled from the device/host by this server (`/proc`, `/sys`, `adb`).
    pub system: SystemMetrics,
    /// Reported by the app's own devtools service.
    pub service: ServiceMetrics,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct SystemMetrics {
    pub available: bool,
    /// Why nothing was sampled, when `available` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    /// Cumulative counters, **not** per-process: namespace-wide on desktop,
    /// device-wide on Android. Diff two reads to get a rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_rx_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_tx_bytes: Option<u64>,
    /// Per thermal zone, as the device reports it — vendors disagree on
    /// whether the raw value is millidegrees or degrees, so it is passed
    /// through unconverted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub thermal: Vec<ThermalDto>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ThermalDto {
    pub zone: String,
    pub raw_value: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ServiceMetrics {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// Best-effort and platform-dependent; absent where the app cannot read
    /// its own RSS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_ms: Option<u64>,
}

/// The structured half of a `screenshot` result; the image itself also rides
/// along as MCP image content so a client renders it directly.
#[derive(Debug, Serialize)]
pub(crate) struct ScreenshotMeta {
    pub session_id: u64,
    /// `service` (the app's own devtools screenshot) or `adb` (a device
    /// `screencap`, which captures the whole screen, not just the app).
    pub source: &'static str,
    pub png_bytes: usize,
    pub png_base64: String,
}

// ── Tools ───────────────────────────────────────────────────────────────────

pub(crate) async fn widget_tree(
    engine: &SessionEngine,
    args: WidgetTreeArgs,
) -> ToolResult<WidgetTreeResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let dump = match super::with_devtools(engine, &snapshot, "widget_tree", |client| {
        client.widget_tree()
    })
    .await
    {
        Ok(dump) => dump,
        Err(err) => return Err(Json(err)),
    };
    let depth = args.depth.unwrap_or(DEFAULT_TREE_DEPTH).min(MAX_TREE_DEPTH);
    let rendered = render_tree(&dump, depth);
    Ok(Json(WidgetTreeResult {
        session_id: snapshot.id.0,
        depth,
        node_count: rendered.node_count,
        truncated: rendered.truncated,
        roots: rendered.roots,
    }))
}

pub(crate) fn performance(
    engine: &SessionEngine,
    args: SessionArgs,
) -> ToolResult<PerformanceResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let frames = engine.frame_ring(snapshot.id).unwrap_or_default();
    let stats = aggregate_frames(&frames);
    let note = stats.is_none().then(|| {
        no_stats_note(
            snapshot.id.0,
            super::state_name(&snapshot.state),
            frames.len(),
        )
    });
    Ok(Json(PerformanceResult {
        session_id: snapshot.id.0,
        sample_count: frames.len(),
        dropped_samples: snapshot.dropped_frames,
        note,
        stats,
    }))
}

pub(crate) async fn metrics(
    engine: &SessionEngine,
    args: SessionArgs,
) -> ToolResult<MetricsResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let latest = engine.latest_metrics(snapshot.id).unwrap_or_default();
    let sampled = latest.cpu.is_some()
        || latest.mem.is_some()
        || latest.net.is_some()
        || !latest.thermal.is_empty();
    let system = SystemMetrics {
        available: sampled,
        unavailable_reason: (!sampled).then(|| system_metrics_gap(&snapshot)),
        cpu_percent: latest.cpu.map(|cpu| cpu.percent),
        rss_bytes: latest.mem.map(|mem| mem.rss_bytes),
        net_rx_bytes: latest.net.map(|net| net.rx_bytes),
        net_tx_bytes: latest.net.map(|net| net.tx_bytes),
        thermal: latest
            .thermal
            .iter()
            .map(|sample| ThermalDto {
                zone: sample.zone_label.clone(),
                raw_value: sample.millideg_c,
            })
            .collect(),
    };

    let service = match super::with_devtools(engine, &snapshot, "metrics_snapshot", |client| {
        client.metrics_snapshot()
    })
    .await
    {
        Ok(snapshot) => ServiceMetrics {
            available: true,
            unavailable_reason: None,
            rss_bytes: snapshot.rss_bytes,
            uptime_ms: Some(snapshot.uptime_ms),
        },
        Err(err) => ServiceMetrics {
            available: false,
            unavailable_reason: Some(err.error),
            rss_bytes: None,
            uptime_ms: None,
        },
    };

    Ok(Json(MetricsResult {
        session_id: snapshot.id.0,
        system,
        service,
    }))
}

/// The screenshot decision chain: the app's own devtools screenshot when it
/// declared the capability, else an Android `screencap` over `adb`, else a
/// refusal that says why.
pub(crate) async fn screenshot(
    engine: &SessionEngine,
    args: SessionArgs,
) -> Result<CallToolResult, Json<ToolError>> {
    let snapshot = resolve_session(engine, args.session_id).map_err(Json)?;

    let declares_screenshot = snapshot
        .devtools_capabilities()
        .is_some_and(|caps| caps.contains(&Capability::Screenshot));

    let (source, png_base64) = if declares_screenshot {
        let Some(client) = engine.devtools_client(snapshot.id) else {
            return Err(Json(super::no_devtools_error(&snapshot)));
        };
        let result = devtools_call(client, "screenshot", |client| client.screenshot())
            .await
            .map_err(Json)?;
        ("service", result.png_base64)
    } else if let Some(serial) = snapshot.target.android_serial() {
        let runner = engine.runner();
        let serial = serial.to_string();
        let captured =
            tokio::task::spawn_blocking(move || adb_screencap_base64(runner.as_ref(), &serial))
                .await
                .map_err(|err| Json(ToolError::new(format!("the screencap task failed: {err}"))))?
                .map_err(Json)?;
        ("adb", captured)
    } else {
        return Err(Json(ToolError::new(format!(
            "screenshot is not available for session {}: the app's devtools service did not \
             declare the screenshot capability, and only an Android session has an adb \
             fallback. Desktop and iOS Simulator screenshots are not supported in v1.",
            snapshot.id
        ))));
    };

    let png_bytes = decode_png(&png_base64).map_err(Json)?;
    let meta = ScreenshotMeta {
        session_id: snapshot.id.0,
        source,
        png_bytes,
        png_base64: png_base64.clone(),
    };
    let structured = rmcp::serde_json::to_value(&meta).map_err(|err| {
        Json(ToolError::new(format!(
            "could not encode the screenshot metadata: {err}"
        )))
    })?;
    // Both halves on purpose: the image content is what an agent UI renders,
    // the structured content is what a programmatic caller reads.
    let mut result = CallToolResult::success(vec![Content::image(png_base64, "image/png")]);
    result.structured_content = Some(structured);
    Ok(result)
}

// ── Rendering and aggregation ───────────────────────────────────────────────

pub(crate) struct RenderedTree {
    pub roots: Vec<TreeNode>,
    pub node_count: usize,
    pub truncated: bool,
}

/// Renders `dump` as nested nodes, descending at most `depth` levels below
/// each root and marking every elided subtree with its dropped child count.
pub(crate) fn render_tree(dump: &WidgetTreeDump, depth: usize) -> RenderedTree {
    let mut rendered = RenderedTree {
        roots: Vec::new(),
        node_count: 0,
        truncated: false,
    };
    rendered.roots = dump
        .roots
        .iter()
        .map(|root| {
            render_node(
                root,
                depth,
                &mut rendered.node_count,
                &mut rendered.truncated,
            )
        })
        .collect();
    rendered
}

fn render_node(
    node: &WidgetNode,
    remaining: usize,
    node_count: &mut usize,
    truncated: &mut bool,
) -> TreeNode {
    *node_count += 1;
    let (children, children_truncated) = if node.children.is_empty() {
        (Vec::new(), None)
    } else if remaining == 0 {
        *truncated = true;
        (Vec::new(), Some(node.children.len()))
    } else {
        (
            node.children
                .iter()
                .map(|child| render_node(child, remaining - 1, node_count, truncated))
                .collect(),
            None,
        )
    };
    TreeNode {
        id: node.id,
        type_name: node.type_name.clone(),
        debug_label: node.debug_label.clone(),
        bounds: node.bounds.map(Rect::from),
        children,
        children_truncated,
    }
}

/// Aggregates a session's retained frame samples, excluding samples the
/// mobile frame gate skipped: a skipped frame's pass durations are all-zero
/// on the wire (`frust-shell-common/src/devtools.rs`'s `publish_frame`), so
/// folding it into the timing/percentile/jank math would deflate the mean
/// and percentiles and inflate `fps_estimate` — the same reasoning the two
/// sibling aggregators apply (`frust-shell-common::perf::FrameStats::summary`,
/// `frust-tui::engine::devtools::perf_stats`). `None` for an empty ring, or
/// for a non-empty ring every one of whose samples was skipped — the caller
/// reports either as "nothing to aggregate", never as zeroed stats.
/// `skipped_frames` and `frame_index_span` still cover the *full* window
/// (a skipped frame was still counted by the app; only its timings are
/// excluded here).
pub(crate) fn aggregate_frames(frames: &[FrameStats]) -> Option<FrameAggregate> {
    let (first, last) = (frames.first()?, frames.last()?);
    let active: Vec<&FrameStats> = frames.iter().filter(|frame| !frame.skipped).collect();
    if active.is_empty() {
        return None;
    }
    let count = active.len();

    let mut sorted_ms: Vec<f64> = active
        .iter()
        .map(|frame| frame.total_us as f64 / 1000.0)
        .collect();
    sorted_ms.sort_by(f64::total_cmp);

    let total_us: u64 = active.iter().map(|frame| frame.total_us).sum();
    let mean_ms = total_us as f64 / count as f64 / 1000.0;
    let jank_frames = active
        .iter()
        .filter(|frame| frame.total_us as f64 / 1000.0 > JANK_BUDGET_MS)
        .count();

    Some(FrameAggregate {
        fps_estimate: if mean_ms > 0.0 {
            round2(1000.0 / mean_ms)
        } else {
            0.0
        },
        frame_ms_mean: round2(mean_ms),
        frame_ms_p50: round2(percentile(&sorted_ms, 50)),
        frame_ms_p95: round2(percentile(&sorted_ms, 95)),
        frame_ms_p99: round2(percentile(&sorted_ms, 99)),
        frame_ms_max: round2(sorted_ms[count - 1]),
        jank_frames,
        jank_percent: round2(jank_frames as f64 * 100.0 / count as f64),
        jank_threshold_ms: JANK_BUDGET_MS,
        skipped_frames: frames.iter().filter(|frame| frame.skipped).count(),
        frame_index_span: last.n.saturating_sub(first.n) + 1,
        sampled_span_ms: us_to_ms(total_us),
        phase_means_ms: PhaseMeans {
            rebuild: phase_mean(&active, |frame| frame.rebuild_us),
            layout: phase_mean(&active, |frame| frame.layout_us),
            paint: phase_mean(&active, |frame| frame.paint_us),
            encode: phase_mean(&active, |frame| frame.encode_us),
            acquire: phase_mean(&active, |frame| frame.acquire_us),
            submit: phase_mean(&active, |frame| frame.submit_us),
        },
    })
}

fn phase_mean(frames: &[&FrameStats], field: impl Fn(&FrameStats) -> u64) -> f64 {
    let total: u64 = frames.iter().map(|frame| field(frame)).sum();
    round2(total as f64 / frames.len() as f64 / 1000.0)
}

/// The `note` text when `performance`'s `stats` is `None`: an empty ring
/// (nothing has arrived over the devtools connection yet) reads very
/// differently from a non-empty ring the frame gate skipped in its
/// entirety (frames arrived; none of them rendered).
fn no_stats_note(session_id: u64, state: &'static str, sample_count: usize) -> String {
    if sample_count == 0 {
        format!(
            "no frame samples yet for session {session_id} (state: {state}). Frame stats \
             arrive over the devtools connection once the app renders; a session that is \
             still launching, or one whose devtools service never started, has none."
        )
    } else {
        format!(
            "{sample_count} frame sample(s) arrived for session {session_id} (state: {state}) \
             but every one was skipped by the mobile frame gate — nothing rendered in this \
             window to aggregate."
        )
    }
}

/// Nearest-rank percentile over an ascending-sorted, non-empty slice.
fn percentile(sorted: &[f64], pct: usize) -> f64 {
    let rank = (sorted.len() * pct).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// Why this session has no system metrics — the honest alternative to
/// reporting zeros (see the module doc).
fn system_metrics_gap(snapshot: &SessionSnapshot) -> String {
    if snapshot.target.android_serial().is_none() {
        return format!(
            "system metrics are Android-only: the process seam exposes no pid for a {} \
             session, so nothing can be sampled for it.",
            snapshot.target.label()
        );
    }
    if !snapshot.metrics_sampling {
        return "the metrics sampler has not started for this session — it begins once the \
                app's pid and package appear in its output."
            .to_string();
    }
    "the metrics sampler is running but has not produced a sample yet.".to_string()
}

/// Captures the device screen through `adb`, base64-encoding it **on the
/// device**.
///
/// The encode has to happen device-side: `ProcessRunner::run` hands back
/// captured stdout as a `String`, which raw PNG bytes do not survive. Any
/// wrapping the device's `base64` applies (toybox's column wrap is version-
/// dependent) is stripped here rather than assumed away.
///
/// Note the scope difference an agent needs to know about, and which the tool
/// result reports as `source: "adb"`: `screencap` captures the whole device
/// screen, including system chrome, not just the app's surface.
fn adb_screencap_base64(runner: &dyn ProcessRunner, serial: &str) -> Result<String, ToolError> {
    let output = runner
        .run("adb", &["-s", serial, "exec-out", "screencap -p | base64"])
        .map_err(|err| {
            ToolError::new(format!(
                "could not run adb for a screencap on {serial}: {err:#}"
            ))
        })?;
    if !output.success {
        return Err(ToolError::new(format!(
            "adb screencap failed on {serial}: {}",
            first_nonempty_line(&output.stderr).unwrap_or("no error output")
        )));
    }
    let encoded: String = output
        .stdout
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if encoded.is_empty() {
        return Err(ToolError::new(format!(
            "adb screencap produced no output on {serial} — the device may have gone away."
        )));
    }
    Ok(encoded)
}

fn first_nonempty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Decodes a base64 PNG payload and returns its byte length, rejecting
/// anything that is not actually a PNG — a device shell error captured as
/// "output" must not reach an agent labelled as an image.
fn decode_png(png_base64: &str) -> Result<usize, ToolError> {
    use base64::Engine as _;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
        .map_err(|err| {
            ToolError::new(format!(
                "the screenshot payload is not valid base64: {err}. \
                 The capture likely returned an error message instead of an image."
            ))
        })?;
    if !bytes.starts_with(&PNG_MAGIC) {
        return Err(ToolError::new(
            "the screenshot payload decoded to something that is not a PNG.",
        ));
    }
    Ok(bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_devtools_protocol::RectPx;
    use frust_drive::process::{FakeProcessRunner, Output};

    fn frame(n: u64, total_us: u64) -> FrameStats {
        FrameStats {
            n,
            total_us,
            rebuild_us: 1_000,
            layout_us: 2_000,
            paint_us: 3_000,
            encode_us: 4_000,
            acquire_us: 5_000,
            submit_us: 6_000,
            skipped: false,
        }
    }

    /// A frame the mobile frame gate skipped: all-zero pass durations, as
    /// `frust-shell-common/src/devtools.rs`'s `publish_frame` puts on the
    /// wire for a skipped `FramePasses`.
    fn skipped_frame(n: u64) -> FrameStats {
        FrameStats {
            n,
            total_us: 0,
            rebuild_us: 0,
            layout_us: 0,
            paint_us: 0,
            encode_us: 0,
            acquire_us: 0,
            submit_us: 0,
            skipped: true,
        }
    }

    fn leaf(id: u64) -> WidgetNode {
        WidgetNode {
            id,
            type_name: "LeafWidget".to_string(),
            debug_label: None,
            bounds: Some(RectPx {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            }),
            children: Vec::new(),
        }
    }

    /// A 1 → 2 → 3 → 4 chain, one child per level.
    fn chain() -> WidgetTreeDump {
        let mut node = leaf(4);
        for id in [3, 2, 1] {
            let mut parent = leaf(id);
            parent.children = vec![node];
            node = parent;
        }
        WidgetTreeDump { roots: vec![node] }
    }

    #[test]
    fn an_empty_ring_aggregates_to_nothing_rather_than_zeros() {
        assert!(aggregate_frames(&[]).is_none());
    }

    #[test]
    fn skipped_frames_are_counted_but_excluded_from_the_timing_math() {
        // Two skipped frames bookend eight real 10ms frames — the skipped
        // frames' all-zero durations must not pull the mean/percentiles
        // down or inflate fps_estimate, but they still count toward
        // skipped_frames and the full window's frame_index_span.
        let mut frames = vec![skipped_frame(0)];
        frames.extend((1..=8).map(|n| frame(n, 10_000)));
        frames.push(skipped_frame(9));

        let stats = aggregate_frames(&frames).expect("eight active samples aggregate");
        assert_eq!(stats.frame_ms_mean, 10.0);
        assert_eq!(stats.frame_ms_p50, 10.0);
        assert_eq!(stats.frame_ms_max, 10.0);
        assert_eq!(stats.fps_estimate, 100.0);
        assert_eq!(stats.jank_frames, 0);
        assert_eq!(stats.jank_percent, 0.0);
        assert_eq!(stats.skipped_frames, 2);
        // Full window: frame 0 through frame 9.
        assert_eq!(stats.frame_index_span, 10);
        assert_eq!(
            stats.phase_means_ms,
            PhaseMeans {
                rebuild: 1.0,
                layout: 2.0,
                paint: 3.0,
                encode: 4.0,
                acquire: 5.0,
                submit: 6.0,
            }
        );
    }

    #[test]
    fn an_all_skipped_window_aggregates_to_nothing_rather_than_zeros() {
        let frames: Vec<FrameStats> = (0..4).map(skipped_frame).collect();
        assert!(aggregate_frames(&frames).is_none());
    }

    #[test]
    fn the_no_stats_note_distinguishes_empty_from_all_skipped() {
        let empty = no_stats_note(1, "running", 0);
        assert!(empty.contains("no frame samples yet"), "{empty}");

        let all_skipped = no_stats_note(1, "running", 4);
        assert!(
            all_skipped.contains("skipped by the mobile frame gate"),
            "{all_skipped}"
        );
        assert!(all_skipped.contains('4'), "{all_skipped}");
        assert_ne!(empty, all_skipped);
    }

    #[test]
    fn the_aggregate_reports_known_percentiles_and_jank() {
        // Ten samples: 8 at 10ms, 1 at 20ms, 1 at 100ms — two over the
        // 16.7ms budget. Nearest-rank puts p50 at index 4 (10ms), p95 at
        // index 9 (100ms), p99 at index 9 (100ms).
        let mut frames: Vec<FrameStats> = (0..8).map(|n| frame(n, 10_000)).collect();
        frames.push(frame(8, 20_000));
        frames.push(frame(9, 100_000));

        let stats = aggregate_frames(&frames).expect("ten samples aggregate");
        assert_eq!(stats.frame_ms_p50, 10.0);
        assert_eq!(stats.frame_ms_p95, 100.0);
        assert_eq!(stats.frame_ms_p99, 100.0);
        assert_eq!(stats.frame_ms_max, 100.0);
        assert_eq!(stats.jank_frames, 2);
        assert_eq!(stats.jank_percent, 20.0);
        assert_eq!(stats.jank_threshold_ms, JANK_BUDGET_MS);
        // mean = (8*10 + 20 + 100) / 10 = 20ms → 50fps.
        assert_eq!(stats.frame_ms_mean, 20.0);
        assert_eq!(stats.fps_estimate, 50.0);
        assert_eq!(stats.sampled_span_ms, 200.0);
        assert_eq!(stats.frame_index_span, 10);
        assert_eq!(stats.skipped_frames, 0);
        assert_eq!(
            stats.phase_means_ms,
            PhaseMeans {
                rebuild: 1.0,
                layout: 2.0,
                paint: 3.0,
                encode: 4.0,
                acquire: 5.0,
                submit: 6.0,
            }
        );
    }

    #[test]
    fn a_single_sample_aggregates_to_itself() {
        let stats = aggregate_frames(&[frame(42, 8_000)]).expect("one sample aggregates");
        assert_eq!(stats.frame_ms_p50, 8.0);
        assert_eq!(stats.frame_ms_p99, 8.0);
        assert_eq!(stats.jank_frames, 0);
        assert_eq!(stats.frame_index_span, 1);
    }

    #[test]
    fn a_gap_in_frame_numbers_widens_the_index_span_beyond_the_sample_count() {
        let stats = aggregate_frames(&[frame(10, 10_000), frame(40, 10_000)]).expect("two samples");
        assert_eq!(stats.frame_index_span, 31);
    }

    #[test]
    fn the_depth_cap_elides_subtrees_and_marks_the_child_count() {
        let rendered = render_tree(&chain(), 1);
        assert!(rendered.truncated);
        assert_eq!(rendered.node_count, 2);
        let root = &rendered.roots[0];
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].children_truncated, Some(1));
        assert!(root.children[0].children.is_empty());
    }

    #[test]
    fn depth_zero_keeps_only_the_roots() {
        let rendered = render_tree(&chain(), 0);
        assert!(rendered.truncated);
        assert_eq!(rendered.node_count, 1);
        assert_eq!(rendered.roots[0].children_truncated, Some(1));
    }

    #[test]
    fn a_depth_past_the_bottom_renders_everything_untruncated() {
        let rendered = render_tree(&chain(), MAX_TREE_DEPTH);
        assert!(!rendered.truncated);
        assert_eq!(rendered.node_count, 4);
    }

    #[test]
    fn the_adb_screencap_encodes_on_the_device_and_strips_wrapping() {
        // A 1x1 PNG, wrapped as toybox's `base64` may wrap it.
        let png = base64_of_smallest_png();
        let wrapped = format!("{}\n{}\n", &png[..8], &png[8..]);
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 exec-out screencap -p | base64",
            Output {
                success: true,
                stdout: wrapped,
                stderr: String::new(),
            },
        );
        let encoded = adb_screencap_base64(&runner, "emulator-5554").expect("the capture succeeds");
        assert_eq!(encoded, png);
        assert!(decode_png(&encoded).expect("valid png") > 0);
    }

    #[test]
    fn a_failed_screencap_reports_the_devices_own_error() {
        let runner = FakeProcessRunner::new().with(
            "adb -s emulator-5554 exec-out screencap -p | base64",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "device offline\n".to_string(),
            },
        );
        let err = adb_screencap_base64(&runner, "emulator-5554").expect_err("offline device");
        assert!(err.error.contains("device offline"), "{}", err.error);
    }

    #[test]
    fn a_missing_adb_is_reported_as_such_not_as_an_empty_capture() {
        let runner = FakeProcessRunner::new();
        let err = adb_screencap_base64(&runner, "emulator-5554").expect_err("no adb registered");
        assert!(err.error.contains("adb"), "{}", err.error);
    }

    #[test]
    fn a_non_png_payload_is_refused_rather_than_returned_as_an_image() {
        use base64::Engine as _;
        let not_png = base64::engine::general_purpose::STANDARD.encode("error: no such thing");
        assert!(decode_png(&not_png).is_err());
        assert!(decode_png("not base64 at all !!!").is_err());
    }

    /// The smallest valid PNG (an 8x1 truncated-but-magic-bearing blob is
    /// enough for the magic check this layer does), base64-encoded.
    fn base64_of_smallest_png() -> String {
        use base64::Engine as _;
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(&[0x0d, 0x0a, 0x1a, 0x0a]);
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }
}
