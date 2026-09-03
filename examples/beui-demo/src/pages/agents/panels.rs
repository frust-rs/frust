//! Agents · Panels — the six cards an agent shows its work in: `code_block`,
//! `file_diff`, `tool_result`, `tool_approval`, `approval_card` and
//! `citations`.
//!
//! One replay button drives the three streaming panels off a single elapsed
//! clock, the way upstream's own `use-tool-result-demo` hook does; the two
//! consent surfaces run their real state machines against this page's state.
//! See [`crate::pages::agents::primitives`] for the component boundary and the
//! shared frame pump both are built on.

use std::time::Duration;

use frust::{AnyView, Column, Component, CrossAxisAlignment, Row, SizedBox, any, component, text};
use frust_beui::agents::approval_card::{ApprovalCardAction, ApprovalCardStatus, approval_card};
use frust_beui::agents::citations::{
    Citation, citation, citation_list, citation_preview, citations,
};
use frust_beui::agents::code_block::{
    CodeBlockStatus, CodeSpan, CodeTokenClass, code_block, code_span,
};
use frust_beui::agents::file_diff::{
    FileDiffLine, FileDiffLineKind, FileDiffStatus, diff_hunk, diff_line, file_diff,
    file_diff_model,
};
use frust_beui::agents::tool_approval::{
    ToolApprovalDecision, ToolApprovalStatus, tool_approval, tool_approval_parameter,
};
use frust_beui::agents::tool_result::{ToolResultKind, ToolResultStatus, tool_result};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::tokens::mono_family;

use super::primitives::{BLOCK_GAP, FrameClock, heading, sub_heading};
use crate::AppState;
use crate::nav::caption;

// ---- Sample data -----------------------------------------------------------

/// How long one snippet line takes to arrive during a replay — upstream's own
/// `code-block.preview.tsx` cadence.
const CODE_LINE_MS: u64 = 260;

/// How long one diff row takes to arrive — upstream's `file-diff.preview.tsx`
/// cadence.
const DIFF_ROW_MS: u64 = 360;

/// How long one terminal output line takes to arrive.
const OUTPUT_LINE_MS: u64 = 420;

/// The tool-approval sequence's own beats, in ms after the decision — upstream's
/// `chat-app-usage.tsx` timings.
const APPROVING_MS: u64 = 450;
/// When the approved run starts.
const RUNNING_MS: u64 = 850;
/// When it finishes.
const COMPLETE_MS: u64 = 1650;

/// How long the approval card's decision stays in flight — upstream's
/// `approval-card-review.preview.tsx` delay.
const SUBMIT_MS: u64 = 700;

/// How many lines [`snippet`] spells, so the replay's progress can be resolved
/// without rebuilding the token list. Asserted against the data in debug.
const SNIPPET_LEN: usize = 13;

/// How many rows [`diff_rows`] holds, for the same reason.
const DIFF_LEN: usize = 5;

/// The terminal payload the tool result streams, one line at a time.
const OUTPUT: [&str; 6] = [
    "$ bun test tests/a11y.test.tsx",
    "bun test v1.3.14",
    "\u{2713} StreamingResponse complete",
    "\u{2713} ToolApproval pending",
    "\u{2713} Citations expanded",
    "49 pass \u{b7} 0 fail",
];

/// One pre-tokenized line of the sample snippet — upstream's `summarize.ts`,
/// hand-classified here because nothing in the port infers syntax (the shiki
/// degradation the code panel records).
fn snippet() -> Vec<Vec<CodeSpan>> {
    use CodeTokenClass::{Function, Keyword, Plain, Punctuation, Str, Type};
    let lines = vec![
        vec![
            code_span("import ", Keyword),
            code_span("{ ", Punctuation),
            code_span("generateText", Function),
            code_span(" } ", Punctuation),
            code_span("from ", Keyword),
            code_span("\"ai\"", Str),
            code_span(";", Punctuation),
        ],
        Vec::new(),
        vec![
            code_span("export async function ", Keyword),
            code_span("summarize", Function),
            code_span("(", Punctuation),
            code_span("input", Plain),
            code_span(": ", Punctuation),
            code_span("string", Type),
            code_span(") {", Punctuation),
        ],
        vec![
            code_span("  ", Plain),
            code_span("const ", Keyword),
            code_span("{ ", Punctuation),
            code_span("text", Plain),
            code_span(" } = ", Punctuation),
            code_span("await ", Keyword),
            code_span("generateText", Function),
            code_span("({", Punctuation),
        ],
        vec![
            code_span("    model", Plain),
            code_span(": ", Punctuation),
            code_span("\"openai/gpt-5\"", Str),
            code_span(",", Punctuation),
        ],
        vec![
            code_span("    prompt", Plain),
            code_span(": ", Punctuation),
            code_span("`Summarize this clearly: ${input}`", Str),
            code_span(",", Punctuation),
        ],
        vec![code_span("  });", Punctuation)],
        Vec::new(),
        vec![
            code_span("  ", Plain),
            code_span("return ", Keyword),
            code_span("{", Punctuation),
        ],
        vec![code_span("    text", Plain), code_span(",", Punctuation)],
        vec![
            code_span("    generatedAt", Plain),
            code_span(": ", Punctuation),
            code_span("new ", Keyword),
            code_span("Date", Type),
            code_span("().", Punctuation),
            code_span("toISOString", Function),
            code_span("(),", Punctuation),
        ],
        vec![code_span("  };", Punctuation)],
        vec![code_span("}", Punctuation)],
    ];
    debug_assert_eq!(lines.len(), SNIPPET_LEN);
    lines
}

/// The literal text one tokenized line spells.
fn line_text(spans: &[CodeSpan]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// The diff the file panel shows — upstream's `file-diff.preview.tsx` rows.
fn diff_rows() -> Vec<FileDiffLine> {
    let rows = vec![
        diff_line(
            FileDiffLineKind::Context,
            "export async function runTask() {",
        )
        .old_line(18)
        .new_line(18),
        diff_line(FileDiffLineKind::Removed, "  return execute(task);").old_line(19),
        diff_line(
            FileDiffLineKind::Added,
            "  const result = await execute(task);",
        )
        .new_line(19),
        diff_line(FileDiffLineKind::Added, "  return normalize(result);").new_line(20),
        diff_line(FileDiffLineKind::Context, "}")
            .old_line(20)
            .new_line(21),
    ];
    debug_assert_eq!(rows.len(), DIFF_LEN);
    rows
}

/// The references the citation blocks show — upstream's own
/// `citations.preview.tsx` list.
fn sources() -> Vec<Citation> {
    vec![
        citation("motion", "Motion documentation")
            .domain("motion.dev")
            .url("https://motion.dev/docs/react"),
        citation("wai", "WAI accessibility patterns")
            .domain("w3.org")
            .url("https://www.w3.org/WAI/ARIA/apg/"),
        citation("react", "React documentation")
            .domain("react.dev")
            .url("https://react.dev/learn"),
    ]
}

// ---- Page state ------------------------------------------------------------

/// Everything this page retains — see [`crate::pages::agents::primitives`] on
/// why it lives behind a component boundary.
pub struct State {
    clock: FrameClock,
    /// How far into the streaming replay the page is.
    run: Duration,
    /// Whether the replay is still delivering.
    replaying: bool,
    /// The code panel's disclosure.
    code_collapsed: bool,
    /// The diff panel's disclosure.
    diff_open: bool,
    /// Each hunk's own disclosure.
    diff_hunks: Vec<bool>,
    /// The tool result's disclosure.
    result_open: bool,
    /// Where the tool approval is.
    approval: ToolApprovalStatus,
    /// Its detail disclosure.
    approval_open: bool,
    /// How long its timed sequence has been running, once one is.
    approval_elapsed: Duration,
    /// Whether that sequence is in flight.
    approval_running: bool,
    /// What the last tool decision was, and which card reported it.
    decision: Option<String>,
    /// Where the approval card is.
    card: ApprovalCardStatus,
    /// The status its in-flight decision resolves to.
    card_pending: Option<ApprovalCardStatus>,
    /// How long that decision has been in flight.
    card_elapsed: Duration,
    /// What the last card action was, and which review reported it.
    card_note: Option<String>,
    /// Which citation chip the pointer is over, 0-based.
    hovered: Option<usize>,
    /// The reference list's disclosure.
    sources_open: bool,
    /// The last chip or row pressed, 1-based.
    activated: Option<usize>,
    /// Which panel's copy affordance was pressed last.
    copied: Option<&'static str>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            clock: FrameClock::default(),
            run: Duration::ZERO,
            replaying: false,
            code_collapsed: false,
            diff_open: true,
            diff_hunks: vec![true],
            result_open: true,
            approval: ToolApprovalStatus::Pending,
            approval_open: true,
            approval_elapsed: Duration::ZERO,
            approval_running: false,
            decision: None,
            card: ApprovalCardStatus::Pending,
            card_pending: None,
            card_elapsed: Duration::ZERO,
            card_note: None,
            hovered: None,
            sources_open: true,
            activated: None,
            copied: None,
        }
    }
}

impl State {
    /// Step the three timed machines by one frame's worth of shell time.
    fn advance(&mut self, delta: Duration) {
        if self.replaying {
            self.run += delta;
            if self.code_lines() == SNIPPET_LEN
                && self.diff_visible() == DIFF_LEN
                && self.output_lines() == OUTPUT.len()
            {
                self.replaying = false;
            }
        }

        if self.approval_running {
            self.approval_elapsed += delta;
            let ms = self.approval_elapsed.as_millis() as u64;
            self.approval = if ms < APPROVING_MS {
                ToolApprovalStatus::Approving
            } else if ms < RUNNING_MS {
                ToolApprovalStatus::Approved
            } else if ms < COMPLETE_MS {
                ToolApprovalStatus::Running
            } else {
                self.approval_running = false;
                ToolApprovalStatus::Complete
            };
        }

        if let Some(resolved) = self.card_pending {
            self.card_elapsed += delta;
            if self.card_elapsed >= Duration::from_millis(SUBMIT_MS) {
                self.card = resolved;
                self.card_pending = None;
            }
        }
    }

    /// How many snippet lines have arrived.
    fn code_lines(&self) -> usize {
        self.delivered(CODE_LINE_MS, SNIPPET_LEN)
    }

    /// How many diff rows have arrived.
    fn diff_visible(&self) -> usize {
        self.delivered(DIFF_ROW_MS, DIFF_LEN)
    }

    /// How many terminal output lines have arrived.
    fn output_lines(&self) -> usize {
        self.delivered(OUTPUT_LINE_MS, OUTPUT.len())
    }

    /// How many of `total` items have arrived at one item per `step_ms`. A
    /// settled page shows all of them; a replay starts at one.
    fn delivered(&self, step_ms: u64, total: usize) -> usize {
        if !self.replaying {
            return total;
        }
        let steps = self.run.as_millis() as u64 / step_ms;
        ((steps + 1) as usize).min(total)
    }

    /// Reset every replay-driven panel to its first frame.
    fn replay(&mut self) {
        self.run = Duration::ZERO;
        self.replaying = true;
    }
}

// ---- Blocks ----------------------------------------------------------------

/// `code_block`: the streamed snippet, syntax-coloured through the caller's own
/// span list, with a collapse morph and a copy affordance.
fn code_panel(state: &State) -> AnyView<State> {
    let lines = snippet();
    let visible = state.code_lines();
    let code = lines[..visible]
        .iter()
        .map(|spans| line_text(spans))
        .collect::<Vec<_>>()
        .join("\n");
    let colored: Vec<Vec<CodeSpan>> = lines[..visible].to_vec();
    let status = if visible < lines.len() {
        CodeBlockStatus::Streaming
    } else {
        CodeBlockStatus::Complete
    };

    any(Column(vec![
        any(sub_heading("code_block")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "Nothing infers syntax here \u{2014} the colours are the caller's own \
             CodeSpan list, one per line, and an unsupplied line stays \
             monochrome. There is no scroll container: a long line is clipped at \
             the right edge and the height cap is a clip, so `follow` pins the \
             window to the last lines instead of scrolling to them. Copy reports \
             the press; the app owns the clipboard.",
        )),
        any(SizedBox(None, Some(12.0))),
        any(code_block::<State>(code)
            .colored(colored)
            .language("typescript")
            .filename("summarize.ts")
            .status(status)
            .highlight_lines(vec![4, 5, 6, 7])
            .max_height(224.0)
            .follow(true)
            .collapsed(state.code_collapsed)
            .on_collapse_change(|s: &mut State, collapsed| s.code_collapsed = collapsed)
            .on_copy(|s: &mut State| s.copied = Some("code_block"))),
        any(state.clock.pump(state.replaying)),
    ]))
}

/// `file_diff`: a synthetic diff model, streamed row by row.
fn diff_panel(state: &State) -> AnyView<State> {
    let rows = diff_rows();
    let visible = state.diff_visible();
    let model = file_diff_model(
        "src/runner.ts",
        vec![diff_hunk("@@ -18,3 +18,4 @@", rows[..visible].to_vec())],
    );
    let status = if visible < rows.len() {
        FileDiffStatus::Streaming
    } else {
        FileDiffStatus::Complete
    };
    let counts = format!(
        "+{} \u{2212}{} across {} row(s)",
        model.additions(),
        model.deletions(),
        visible
    );

    any(Column(vec![
        any(sub_heading("file_diff")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "The panel takes a parsed model, never a patch string \u{2014} nothing \
             here parses unified diff. Rows are monochrome unless the caller \
             colours them, the disclosure is controlled end to end (the panel \
             never opens or closes itself on a status change), and the copy \
             affordance sits in the header so it stays reachable while the file \
             is collapsed.",
        )),
        any(SizedBox(None, Some(12.0))),
        any(file_diff::<State>(model)
            .status(status)
            .open(state.diff_open)
            .open_hunks(state.diff_hunks.clone())
            .max_height(150.0)
            .on_open_change(|s: &mut State, open| s.diff_open = open)
            .on_hunk_toggle(|s: &mut State, index| {
                if let Some(slot) = s.diff_hunks.get_mut(index) {
                    *slot = !*slot;
                }
            })
            .on_copy(|s: &mut State| s.copied = Some("file_diff"))),
        any(SizedBox(None, Some(8.0))),
        any(caption(counts)),
        any(state.clock.pump(state.replaying)),
    ]))
}

/// A monospace payload block, the shape every tool result's output takes.
fn output(body: impl Into<String>) -> frust::TextView {
    text(body.into()).size(12.0).family(mono_family())
}

/// `tool_result`: a run that streams to success, plus a failed and a cancelled
/// instance.
fn result_panels(state: &State) -> AnyView<State> {
    let visible = state.output_lines();
    let running = visible < OUTPUT.len();
    let body = OUTPUT[..visible].join("\n");

    let live = tool_result::<State, _>(
        "terminal.run",
        if running {
            "Running accessibility tests"
        } else {
            "Tests passed"
        },
        output(body),
    )
    .kind(ToolResultKind::Terminal)
    .status(if running {
        ToolResultStatus::Running
    } else {
        ToolResultStatus::Success
    })
    .open(state.result_open)
    .max_height(150.0)
    .on_open_change(|s: &mut State, open| s.result_open = open)
    .on_copy(|s: &mut State| s.copied = Some("tool_result"))
    .on_retry(|s: &mut State| s.replay());
    let live = if running {
        live.meta("Live")
    } else {
        live.duration(Duration::from_millis(2900))
    };

    any(Column(vec![
        any(sub_heading("tool_result")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "Running, success, error and cancelled. Only the status glyph morphs \
             \u{2014} upstream rolls every label through its swap cell, which a leaf \
             panel shaping its own runs cannot reach. There is no scroll \
             container, so a payload past the height cap is clipped, and the \
             disclosure is the caller's.",
        )),
        any(SizedBox(None, Some(12.0))),
        any(live),
        any(SizedBox(None, Some(12.0))),
        any(
            tool_result::<State, _>(
                "http.request",
                "Request failed",
                output("{\n  \"error\": \"rate_limit_exceeded\",\n  \"retryAfter\": 30,\n  \"requestId\": \"req_8f21\"\n}"),
            )
            .kind(ToolResultKind::Request)
            .status(ToolResultStatus::Error)
            .meta("429")
            .max_height(150.0)
            .on_retry(|s: &mut State| s.replay()),
        ),
        any(SizedBox(None, Some(12.0))),
        any(
            tool_result::<State, _>(
                "terminal.run",
                "Checkout checks were not run",
                output("Permission was not granted. No command was run."),
            )
            .kind(ToolResultKind::Terminal)
            .status(ToolResultStatus::Cancelled)
            .max_height(150.0),
        ),
        any(state.clock.pump(state.replaying)),
    ]))
}

/// `tool_approval`: the real consent surface, reporting its identity.
fn approval_panel(state: &State) -> AnyView<State> {
    let decision = match &state.decision {
        Some(line) => line.clone(),
        None => "No decision yet.".to_string(),
    };

    any(Column(vec![
        any(sub_heading("tool_approval")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "One decision per consent episode: a press latches until the status, \
             the head text, the offered buttons or the id change. The decision \
             is reported with the identity of the card that carried it \u{2014} \
             on_decision_with_id hands back Some(id) only because .id() is set \
             below, and there is deliberately no derived fallback, since a title \
             is not an identity. Parameter values are wrapped mono runs, not \
             code blocks, and an overflowing one paints a trailing ellipsis \
             rather than eliding consent-bearing text silently.",
        )),
        any(SizedBox(None, Some(12.0))),
        any(tool_approval::<State>("terminal.run")
            .title(if state.approval.is_pending() {
                "Run focused checkout checks?"
            } else {
                "Terminal access"
            })
            .description(
                "The agent needs permission to run the validation and \
                     accessibility suites in the current workspace.",
            )
            .parameters(vec![
                tool_approval_parameter("Command", "bun test checkout --coverage"),
                tool_approval_parameter("Scope", "Current workspace"),
            ])
            .status(state.approval)
            .open(state.approval_open)
            .always_allow(true)
            .id("approval-terminal-run")
            .on_open_change(|s: &mut State, open| s.approval_open = open)
            .on_decision_with_id(
                |s: &mut State, decision: ToolApprovalDecision, id: Option<String>| {
                    s.decision = Some(format!(
                        "{} \u{2014} reported by {}",
                        decision.label(),
                        id.unwrap_or_else(|| "an unidentified card".to_string())
                    ));
                    match decision {
                        ToolApprovalDecision::Deny => {
                            s.approval = ToolApprovalStatus::Denied;
                            s.approval_running = false;
                        }
                        _ => {
                            s.approval = ToolApprovalStatus::Approving;
                            s.approval_elapsed = Duration::ZERO;
                            s.approval_running = true;
                        }
                    }
                },
            )),
        any(SizedBox(None, Some(10.0))),
        any(Row(vec![
            any(caption(format!(
                "{decision} Status: {}.",
                state.approval.label()
            ))),
            any(SizedBox(Some(12.0), None)),
            any(button("Reset", |s: &mut State| {
                s.approval = ToolApprovalStatus::Pending;
                s.approval_running = false;
                s.approval_elapsed = Duration::ZERO;
                s.decision = None;
            })
            .tone(ButtonTone::Ghost)
            .size(ButtonSize::Sm)),
        ])
        .cross_axis(CrossAxisAlignment::Center)),
        any(state.clock.pump(state.approval_running)),
    ]))
}

/// `approval_card`: the review surface, with the keyed detail block that is
/// what makes it answerable at all.
fn card_panel(state: &State) -> AnyView<State> {
    let note = match &state.card_note {
        Some(line) => line.clone(),
        None => "No action yet.".to_string(),
    };
    let detail = Column(vec![
        any(caption("Release  \u{b7}  approval-card")),
        any(SizedBox::<State>(None, Some(4.0))),
        any(caption("Checks   \u{b7}  4 passed")),
        any(SizedBox::<State>(None, Some(4.0))),
        any(caption("Visibility  \u{b7}  Public registry")),
    ]);

    any(Column(vec![
        any(sub_heading("approval_card")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "The review variant; upstream's question wizard is a multi-step form \
             over the catalog's own controls and is deliberately not ported. The \
             detail block is passed through content_keyed \u{2014} a bare .content \
             leaves the three decision buttons disabled by design, because a \
             child's repaints say nothing about whether what you are consenting \
             to changed. Answering collapses the card into its recorded result; \
             Dismiss is never latched and is offered in every status, so it \
             resets this demo rather than answering it.",
        )),
        any(SizedBox(None, Some(12.0))),
        any(approval_card::<State>()
            .title("Publish the component update?")
            .description(
                "The agent has prepared the release and is waiting for your \
                     decision.",
            )
            .content_keyed("release-2026-09-03", detail)
            .status(state.card)
            .request_changes(true)
            .reject(true)
            .dismissible(true)
            .id("approval-card-release")
            .result(match state.card {
                ApprovalCardStatus::Approved => "Publishing was approved.",
                ApprovalCardStatus::ChangesRequested => "The agent will wait for revision notes.",
                ApprovalCardStatus::Rejected => "Publishing was declined.",
                _ => "Waiting on the release decision.",
            })
            .on_action_with_id(
                |s: &mut State, action: ApprovalCardAction, id: Option<String>| {
                    s.card_note = Some(format!(
                        "{} \u{2014} reported by {}",
                        action.label(),
                        id.unwrap_or_else(|| "an unidentified review".to_string())
                    ));
                    let resolved = match action {
                        ApprovalCardAction::Approve => Some(ApprovalCardStatus::Approved),
                        ApprovalCardAction::RequestChanges => {
                            Some(ApprovalCardStatus::ChangesRequested)
                        }
                        ApprovalCardAction::Reject => Some(ApprovalCardStatus::Rejected),
                        ApprovalCardAction::Dismiss => None,
                    };
                    match resolved {
                        Some(status) => {
                            s.card = ApprovalCardStatus::Submitting;
                            s.card_pending = Some(status);
                            s.card_elapsed = Duration::ZERO;
                        }
                        None => {
                            s.card = ApprovalCardStatus::Pending;
                            s.card_pending = None;
                        }
                    }
                },
            )),
        any(SizedBox(None, Some(10.0))),
        any(caption(format!("{note} Status: {}.", state.card.label()))),
        any(state.clock.pump(state.card_pending.is_some())),
    ]))
}

/// `citations`: the numbered chip row, its preview, and the reference list.
fn citation_panel(state: &State) -> AnyView<State> {
    let items = sources();
    let preview: AnyView<State> = match state.hovered.and_then(|index| {
        items
            .get(index)
            .map(|citation| citation_preview(citation.clone(), index + 1))
    }) {
        Some(panel) => any(panel),
        None => any(caption("Hover a chip above to preview its reference here.")),
    };
    let activated = match state.activated {
        Some(index) => format!("Last press: source {index}."),
        None => "Nothing pressed yet.".to_string(),
    };

    any(Column(vec![
        any(sub_heading("citations")),
        any(SizedBox(None, Some(6.0))),
        any(caption(
            "Numbered chips, not stacked favicons \u{2014} a plugin-tier widget has \
             no network to fetch one with. The chip row publishes the hovered \
             chip's window rect for the catalog's anchored overlay host, but that \
             host needs bounded constraints and must not sit inside a scroll \
             view, which the gallery's page slot is; so this page reads the \
             hover through on_hover_change and mounts the same preview panel \
             inline instead. A press reports its index and the app decides what \
             to do with it \u{2014} there is no document here to jump into.",
        )),
        any(SizedBox(None, Some(12.0))),
        any(text(
            "Use layout-aware motion for newly appended results, and preserve \
                 accessible disclosure behaviour as the list grows.",
        )
        .size(14.0)),
        any(SizedBox(None, Some(8.0))),
        any(citations::<State>(items.clone())
            .on_hover_change(|s: &mut State, index| s.hovered = index)
            .on_activate(|s: &mut State, index| s.activated = Some(index + 1))),
        any(SizedBox(None, Some(12.0))),
        preview,
        any(SizedBox(None, Some(12.0))),
        any(citation_list::<State>(items)
            .title("Sources")
            .open(state.sources_open)
            .on_open_change(|s: &mut State, open| s.sources_open = open)
            .on_activate(|s: &mut State, index| s.activated = Some(index + 1))),
        any(SizedBox(None, Some(8.0))),
        any(caption(activated)),
    ]))
}

// ---- The page --------------------------------------------------------------

/// The page's component: owns [`State`], steps the timed machines, and
/// assembles the six panels.
struct Panels;

impl Component for Panels {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> AnyView<State> {
        let delta = state.clock.tick();
        state.advance(delta);

        let copied = match state.copied {
            Some(panel) => format!("Copy was last pressed on {panel}."),
            None => "No copy affordance pressed yet.".to_string(),
        };

        any(Column(vec![
            any(heading("Agents \u{b7} Panels")),
            any(SizedBox(None, Some(8.0))),
            any(caption(
                "The cards an agent shows its work in. The three streaming \
                 panels replay off one clock; the two consent surfaces run their \
                 real state machines against this page's state.",
            )),
            any(SizedBox(None, Some(12.0))),
            any(Row(vec![
                any(button("Replay the stream", |s: &mut State| s.replay())
                    .tone(ButtonTone::Outline)
                    .size(ButtonSize::Sm)),
                any(SizedBox(Some(12.0), None)),
                any(caption(copied)),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
            any(SizedBox(None, Some(BLOCK_GAP))),
            code_panel(state),
            any(SizedBox(None, Some(BLOCK_GAP))),
            diff_panel(state),
            any(SizedBox(None, Some(BLOCK_GAP))),
            result_panels(state),
            any(SizedBox(None, Some(BLOCK_GAP))),
            approval_panel(state),
            any(SizedBox(None, Some(BLOCK_GAP))),
            card_panel(state),
            any(SizedBox(None, Some(BLOCK_GAP))),
            citation_panel(state),
        ]))
    }
}

/// The Agents · Panels page.
pub fn page() -> AnyView<AppState> {
    any(component(Panels))
}
