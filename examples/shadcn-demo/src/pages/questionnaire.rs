//! Questionnaire: a five-item flow mixing single-choice, multi-choice, a
//! required item, a free-text item and a skippable one, driven entirely from
//! three fields of this page's own state.
//!
//! The widget is controlled: it reports a requested answer, a requested index
//! and the submit, and never writes anything itself — `on_answer` stores into
//! `answers[e.item]`, `on_navigate` stores `current`, `on_submit` flips
//! `submitted`. Completion swaps the flow for a summary of what was recorded.

use frust::{Column, SizedBox, View, any, text};
use frust_shadcn::{
    ButtonVariant, QuestionnaireAnswer, QuestionnaireAnswerEvent, QuestionnaireItem,
    QuestionnaireShortcuts, QuestionnaireStatus, button, questionnaire, questionnaire_choice,
    questionnaire_item, separator,
};

use crate::AppState;

pub struct State {
    pub current: usize,
    pub answers: Vec<QuestionnaireAnswer>,
    pub submitted: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            current: 0,
            answers: vec![QuestionnaireAnswer::default(); ITEM_COUNT],
            submitted: false,
        }
    }
}

/// How many items [`items`] builds — the answer list is index-parallel to it.
const ITEM_COUNT: usize = 5;

/// The flow itself: two single-choice items (one required), one multi-choice,
/// one free-text, and a closing optional one.
fn items() -> Vec<QuestionnaireItem> {
    vec![
        questionnaire_item("role", "What do you build with Frust?")
            .description("Pick the closest match.")
            .choices(vec![
                questionnaire_choice("mobile", "Mobile apps"),
                questionnaire_choice("desktop", "Desktop apps"),
                questionnaire_choice("both", "Both"),
                questionnaire_choice("watching", "Neither \u{2014} just watching")
                    .description("No judgement."),
            ])
            .required(true),
        questionnaire_item("systems", "Which design systems do you use?")
            .description("Choose as many as apply.")
            .choices(vec![
                questionnaire_choice("shadcn", "shadcn/ui"),
                questionnaire_choice("material", "Material 3"),
                questionnaire_choice("cupertino", "Cupertino"),
                questionnaire_choice("glyph", "Glyph"),
                questionnaire_choice("own", "Something of my own")
                    .description("An out-of-tree catalog."),
            ])
            .multiple(true),
        questionnaire_item("missing", "What is missing from this catalog?")
            .description("Free text \u{2014} Enter confirms from inside the field.")
            .input("A component, a prop, an interaction\u{2026}"),
        questionnaire_item("motion", "How do the exit animations feel?")
            .choices(vec![
                questionnaire_choice("fast", "Too fast"),
                questionnaire_choice("right", "About right"),
                questionnaire_choice("slow", "Too slow"),
                questionnaire_choice("disabled", "I run with reduced motion")
                    .description("The ramps collapse to a jump."),
            ])
            .required(true),
        questionnaire_item("contact", "May we follow up?")
            .description("Optional \u{2014} Skip is offered because it is not required.")
            .choices(vec![
                questionnaire_choice("yes", "Yes"),
                questionnaire_choice("no", "No"),
            ]),
    ]
}

/// The recorded answer for `index`, rendered as one summary line.
fn summary_line(item: &QuestionnaireItem, answer: &QuestionnaireAnswer) -> String {
    let status = match answer.status() {
        QuestionnaireStatus::Answered => "answered",
        QuestionnaireStatus::Skipped => "skipped",
        QuestionnaireStatus::Unanswered => "unanswered",
    };
    let mut recorded = answer.values.join(", ");
    if !answer.text.trim().is_empty() {
        if !recorded.is_empty() {
            recorded.push_str(" / ");
        }
        recorded.push_str(answer.text.trim());
    }
    if recorded.is_empty() {
        recorded.push('\u{2014}');
    }
    format!("{} [{status}]: {recorded}", item.name())
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let items = items();
    let current = state.current.min(items.len().saturating_sub(1));
    let answers = state.answers.clone();

    let mut children: Vec<frust::AnyView<AppState>> = vec![
        any(crate::nav::heading("Questionnaire")),
        any(SizedBox(None, Some(16.0))),
        any(crate::nav::caption(
            "Click a choice once so the flow holds focus, then drive it from the \
             keyboard: a/b/c/d select, arrows rove the choices, Enter confirms \
             (including from inside the text field). Next stays blocked until an \
             item is answered or skipped \u{2014} an optional item blocks too, \
             which is why it offers Skip.",
        )),
        any(SizedBox(None, Some(16.0))),
    ];

    if state.submitted {
        let mut lines: Vec<frust::AnyView<AppState>> = vec![
            any(text("Submitted \u{2014} here is what the callbacks recorded:").size(14.0)),
            any(SizedBox(None, Some(8.0))),
        ];
        for (index, item) in items.iter().enumerate() {
            let answer = answers.get(index).cloned().unwrap_or_default();
            lines.push(any(text(summary_line(item, &answer)).size(13.0)));
            lines.push(any(SizedBox(None, Some(4.0))));
        }
        lines.push(any(SizedBox(None, Some(8.0))));
        lines.push(any(button("Start over", |s: &mut AppState| {
            s.questionnaire = State::default();
        })));
        children.push(any(Column(lines)));
    } else {
        children.push(any(questionnaire(items, current, answers)
            .shortcuts(QuestionnaireShortcuts::Letters)
            .on_answer(|s: &mut AppState, e: QuestionnaireAnswerEvent| {
                if let Some(slot) = s.questionnaire.answers.get_mut(e.item) {
                    *slot = e.answer;
                }
            })
            .on_navigate(|s: &mut AppState, index: usize| {
                s.questionnaire.current = index;
            })
            .on_submit(|s: &mut AppState| {
                s.questionnaire.submitted = true;
            })));
        children.push(any(SizedBox(None, Some(16.0))));
        children.push(any(separator()));
        children.push(any(SizedBox(None, Some(8.0))));
        children.push(any(crate::nav::caption(format!(
            "Item {} of {ITEM_COUNT}. The widget never writes state: every choice, \
             skip and navigation above came back through on_answer/on_navigate.",
            current + 1
        ))));
        children.push(any(SizedBox(None, Some(8.0))));
        children.push(any(button("Jump to the last item", |s: &mut AppState| {
            s.questionnaire.current = ITEM_COUNT - 1;
        })
        .variant(ButtonVariant::Outline)));
    }

    Column(children)
}
