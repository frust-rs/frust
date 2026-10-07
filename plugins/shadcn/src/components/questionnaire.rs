//! `questionnaire`: shadcn's multi-step question flow — one item at a time, each
//! a prompt plus N choices and/or a free-text answer, over a
//! Previous/Skip/Next/Submit action row.
//!
//! Sources (shadcn/ui v4, rev `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`,
//! retrieved 2026-08-17):
//!
//! - **Behavior**: `packages/react/src/questionnaire/` — the headless package
//!   (`README.md`, `types.ts`, `collection.ts`, `utils.ts`, `context.ts`,
//!   `components.tsx`, and the four `use-questionnaire-*` hooks).
//! - **Chrome**: `apps/v4/registry/bases/base/ui/questionnaire.tsx` plus the
//!   `cn-questionnaire-*` class bodies in
//!   `apps/v4/registry/styles/style-nova.css`.
//!
//! | class | here |
//! |---|---|
//! | `flex w-full flex-col gap-4` (root) | one column at [`QUESTIONNAIRE_GAP`] |
//! | `text-xs font-medium text-muted-foreground` (progress) | a `Question x of n` run |
//! | `text-base leading-snug font-medium` (title) | a wrapping [`frust::text`] child |
//! | `[&:not(:has(~description))]:mb-4` | [`TITLE_MARGIN_BOTTOM`] when the item has no description |
//! | `text-sm text-muted-foreground` (description) | a wrapping child at `on_surface_variant` |
//! | `grid gap-2` (choices) | a column at [`CHOICES_GAP`] |
//! | `min-h-11 rounded-lg border px-3 py-2.5 gap-2.5` (choice) | [`QUESTIONNAIRE_CHOICE_MIN_HEIGHT`] + [`ShadcnRadius::lg`] |
//! | `dark:bg-input/20` / `hover:bg-muted/50` / `data-checked:bg-muted` | the three choice fills |
//! | `data-checked:border-primary/40`, `data-invalid:border-destructive` | the choice border ladder |
//! | `size-4 rounded-[4px]` indicator, `rounded-full` when radio | [`INDICATOR_SIZE`] |
//! | `size-2` dot / `size-3.5` check | [`INDICATOR_DOT_SIZE`] / [`INDICATOR_CHECK_SIZE`] |
//! | `size-5 rounded-md border font-mono text-[0.625rem]` (shortcut) | [`QUESTIONNAIRE_SHORTCUT_SIZE`] |
//! | `h-8 rounded-lg border px-2.5 py-1` (input) | [`QUESTIONNAIRE_INPUT_HEIGHT`] |
//! | `mt-2 text-sm text-destructive` (error) | [`ERROR_MARGIN_TOP`] over `error` |
//! | `grid grid-cols-[1fr_auto_auto] gap-2` (actions) | Previous start, Skip/Next-or-Submit end |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] |
//!
//! # Controlled, never self-mutating
//!
//! Three props are the whole model and the app owns all three: the current item
//! index, the per-item [`QuestionnaireAnswer`] list, and the item definitions.
//! Every interaction *reports* rather than applies —
//! [`on_answer`](QuestionnaireView::on_answer) carries the requested answer for
//! one item, [`on_navigate`](QuestionnaireView::on_navigate) the requested item
//! index, [`on_submit`](QuestionnaireView::on_submit) the finish — and the widget
//! leaves its own copies alone until the next `rebuild` feeds the confirmed
//! values back down (`docs/CODE_STANDARDS.md`'s Interaction Semantics). Skipping
//! the last item fires `on_answer` and then `on_submit` in that order, so the
//! app's submit handler already sees the skip it just applied.
//!
//! # Validation is the source's, including for optional items
//!
//! `use-questionnaire-item.ts` computes `valid` as *intentionally skipped* **or**
//! *answered* — an item that is merely unanswered fails validation whether or not
//! it is `required`, which is why the optional error line reads "Choose an answer
//! or skip this question." A Next/Submit press on an unvalidated item therefore
//! latches the error line ([`QuestionnaireWidget::validation_attempted`], the
//! port of the source's `validationAttempted`) and reports nothing. `required`'s
//! own job is narrower: it hides Skip, so an optional item has an escape and a
//! required one does not.
//!
//! # One widget, N targets
//!
//! Choices, the shortcut badges and the four action buttons are painted by this
//! widget rather than each owning a `ChildPod`, the same call `radio_group` makes
//! — it is what lets the roving focus, the shortcut keys and the visibility rules
//! live in one place. `ChildPod`s carry only what has to **wrap or edit**: the
//! title, the description, each choice's label/description column, and the
//! free-text field.
//!
//! Because those pods exist, the container rules bind: a `Move` is routed to the
//! field first and this widget claims hover only afterwards, so a claim of its own
//! never starves the child under the pointer.
//!
//! # What the port deliberately does not carry
//!
//! - **No collection warnings.** `collection.ts` exists to diff `Root.items`
//!   against the rendered `Questionnaire.Item` tree and `console.warn` on drift.
//!   Here the items *are* the data — one [`QuestionnaireItem`] list drives both —
//!   so there is no second tree to drift from.
//! - **No per-item `disabled`, no external `invalid`.** Both are collection-level
//!   knobs upstream uses to model conditional flows; an app that wants an item
//!   gone leaves it out of the list it passes.
//! - **The free-text answer and the choices are one value.** Upstream keeps a
//!   typed-but-unselected input's text in the DOM and merely drops its `name`, so
//!   it is not submitted. Here [`QuestionnaireAnswer::text`] *is* the submitted
//!   value, so a single-select item clears the other side when one is used; a
//!   `multiple` item keeps both.
//! - **No motion.** The source's chrome is `transition-colors` only and this port
//!   paints its states directly, so there is no animation for
//!   `Theme.motion.reduce_motion` to collapse.
//! - **`min-w-[14ch]` on the progress line is dropped** — it exists to stop the
//!   line reflowing as the digits change, and nothing here reflows off it.
//!
//! [`ShadcnRadius::lg`]: crate::ShadcnRadius::lg

use std::rc::Rc;

use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    CursorIcon, ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, Key,
    KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect,
    Role, RoundedRect, SemanticsCtx, Shape, Size, ThemeTextColor, ThemeTextType, Toggled, View,
    Widget, any, build_child, erase_callback, erase_callback_arg, rebuild_children,
    route_event_single, teardown_child, visit_children,
};
use frust::{ColorScheme, CrossAxisAlignment, SizedBox, Theme, column, text, text_input};

use crate::hit::presses;
use crate::style::{self, PATH_TOLERANCE};
use crate::text::{LabelRun, SHAPING_INK, themed_family};
use crate::tokens::{ShadcnRadius, ShadcnTokens, color_scheme_light, mono_family};

// ---- Metrics ---------------------------------------------------------------

/// `gap-4` — the gap between the root's sections and between an item's own
/// prompt/choices/error blocks.
pub const QUESTIONNAIRE_GAP: f64 = 16.0;

/// `min-h-11` — a choice row's floor, and the one metric in this component that
/// clears the mobile tap-target convention [`style::HEIGHT_DEFAULT`] documents.
pub const QUESTIONNAIRE_CHOICE_MIN_HEIGHT: f64 = 44.0;

/// `size-5` — the shortcut badge's edge.
pub const QUESTIONNAIRE_SHORTCUT_SIZE: f64 = 20.0;

/// `h-8` — the free-text field's height.
pub const QUESTIONNAIRE_INPUT_HEIGHT: f64 = style::HEIGHT_SM;

/// `mb-4` — extra space under a title whose item authors no description
/// (`[&:not(:has(~[data-slot=questionnaire-description]))]:mb-4`).
const TITLE_MARGIN_BOTTOM: f64 = 16.0;

/// `gap-2` — between choices, and between the last choice and the free-text
/// field.
const CHOICES_GAP: f64 = 8.0;

/// `px-3` — a choice row's horizontal padding.
const CHOICE_PAD_X: f64 = 12.0;
/// `py-2.5` — a choice row's vertical padding.
const CHOICE_PAD_Y: f64 = 10.0;
/// `gap-2.5` — between a choice's indicator, its content and its badge.
const CHOICE_GAP: f64 = 10.0;

/// `size-4` — the choice indicator's edge.
const INDICATOR_SIZE: f64 = 16.0;
/// `rounded-[4px]` — the checkbox indicator's corner (an arbitrary Tailwind
/// value, so a constant rather than a radius-scale step). A radio indicator is
/// `rounded-full` instead.
const INDICATOR_RADIUS: f64 = 4.0;
/// `size-2` — the radio indicator's dot.
const INDICATOR_DOT_SIZE: f64 = 8.0;
/// `size-3.5` — the checkbox indicator's check glyph.
const INDICATOR_CHECK_SIZE: f64 = 14.0;
/// `translate-y-[--spacing(0.45)]` — the optical nudge that drops the indicator
/// and the badge onto the first line of the choice label.
const INDICATOR_NUDGE: f64 = 1.8;

/// `text-[0.625rem]` — the shortcut badge's mono text size.
const SHORTCUT_TEXT: f64 = 10.0;

/// `px-2.5` — the free-text field's horizontal padding.
const INPUT_PAD_X: f64 = 10.0;
/// `py-1` — the free-text field's vertical padding.
const INPUT_PAD_Y: f64 = 4.0;

/// `mt-2` — the error line's top margin.
const ERROR_MARGIN_TOP: f64 = 8.0;

/// `px-4` on the action buttons (`buttonVariants` size `default`).
const BUTTON_PAD_X: f64 = 16.0;
/// `h-9` on the action buttons, and therefore the action row's height.
const BUTTON_HEIGHT: f64 = style::HEIGHT_DEFAULT;
/// `gap-2` between the action row's columns.
const ACTIONS_GAP: f64 = 8.0;

/// Width used when the incoming constraints are horizontally unbounded — the
/// source's own demo caps the flow at `max-w-md`.
const UNBOUNDED_WIDTH: f64 = 360.0;

/// `dark:bg-input/20` — a resting choice's dark-mode wash (light mode paints
/// none: the class list is `bg-transparent`).
const CHOICE_DARK_FILL_ALPHA: f32 = 0.20;
/// `hover:bg-muted/50`.
const CHOICE_HOVER_ALPHA: f32 = 0.50;
/// `data-checked:border-primary/40`.
const CHOICE_CHECKED_BORDER_ALPHA: f32 = 0.40;
/// `dark:bg-input/30` — the indicator's and the field's dark-mode wash.
const DARK_FILL_ALPHA: f32 = 0.30;

/// Side of the lucide icon viewBox the check path's coordinates are authored in
/// (`lucide-react`'s icons are all `0 0 24 24`).
const ICON_VIEWBOX: f64 = 24.0;
/// The check path's stroke width in viewBox units (`stroke-width="2"`, lucide's
/// uniform value).
const CHECK_STROKE_VIEWBOX: f64 = 2.0;

/// Letters mode's alphabet (`getShortcutKeys`'s `String.fromCharCode(65 + i)`).
const SHORTCUT_LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
/// Numbers mode's digits (`getShortcutKeys`'s `String(i + 1)`, 1–9).
const SHORTCUT_NUMBERS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

// ---- Data ------------------------------------------------------------------

/// Which keyboard shortcut alphabet labels a question's choices —
/// `Root.shortcuts` upstream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuestionnaireShortcuts {
    /// No badges and no shortcut keys (the source's `shortcuts` left unset).
    #[default]
    None,
    /// `A`, `B`, `C`, … — matched case-insensitively against the typed key.
    Letters,
    /// `1`, `2`, … `9`.
    Numbers,
}

impl QuestionnaireShortcuts {
    /// The badge for the `index`-th **enabled** choice, if this mode has one.
    fn label(self, index: usize) -> Option<String> {
        let key = match self {
            QuestionnaireShortcuts::None => return None,
            QuestionnaireShortcuts::Letters => SHORTCUT_LETTERS.get(index)?,
            QuestionnaireShortcuts::Numbers => SHORTCUT_NUMBERS.get(index)?,
        };
        Some((*key).to_string())
    }

    /// Normalize a typed character into a badge label, or `None` when it names
    /// no shortcut in this mode (`getShortcutFromKey`).
    fn shortcut_for_key(self, typed: &str) -> Option<String> {
        match self {
            QuestionnaireShortcuts::None => None,
            QuestionnaireShortcuts::Letters => {
                let upper = typed.to_uppercase();
                SHORTCUT_LETTERS.contains(&upper.as_str()).then_some(upper)
            }
            QuestionnaireShortcuts::Numbers => {
                SHORTCUT_NUMBERS.contains(&typed).then(|| typed.to_string())
            }
        }
    }
}

/// How far an item has got — the source's `QuestionnaireItemStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuestionnaireStatus {
    /// Nothing chosen and nothing typed.
    #[default]
    Unanswered,
    /// At least one choice selected, or free text entered.
    Answered,
    /// Explicitly skipped through the Skip button.
    Skipped,
}

/// One item's answer: the chosen choice values, the free text, and whether the
/// item was skipped.
///
/// The app owns the list of these and feeds it back through
/// [`questionnaire`]'s `answers` argument; the widget only ever *reports* a
/// requested replacement (see the [module docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuestionnaireAnswer {
    /// The selected [`QuestionnaireChoice::value`]s — at most one unless the item
    /// is [`multiple`](QuestionnaireItem::multiple).
    pub values: Vec<String>,
    /// The free-text answer, when the item authors an
    /// [`input`](QuestionnaireItem::input).
    pub text: String,
    /// Whether the item was skipped. Set by the Skip button, cleared by any
    /// answer.
    pub skipped: bool,
}

impl QuestionnaireAnswer {
    /// This answer's [`QuestionnaireStatus`]: `Skipped` wins, then any selected
    /// value or non-blank text makes it `Answered`.
    pub fn status(&self) -> QuestionnaireStatus {
        if self.skipped {
            QuestionnaireStatus::Skipped
        } else if !self.values.is_empty() || !self.text.trim().is_empty() {
            QuestionnaireStatus::Answered
        } else {
            QuestionnaireStatus::Unanswered
        }
    }
}

/// One selectable answer inside a [`QuestionnaireItem`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionnaireChoice {
    value: String,
    label: String,
    description: Option<String>,
    disabled: bool,
}

/// Create a choice labelled `label` that reports `value` when chosen.
pub fn questionnaire_choice(
    value: impl Into<String>,
    label: impl Into<String>,
) -> QuestionnaireChoice {
    QuestionnaireChoice {
        value: value.into(),
        label: label.into(),
        description: None,
        disabled: false,
    }
}

impl QuestionnaireChoice {
    /// Set the muted second line under the label.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Make this choice unselectable: dimmed, inert, skipped by the roving focus
    /// and given no shortcut badge (`getShortcutByChoiceValue` skips disabled
    /// choices before assigning keys).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this choice reports.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// One question in a [`questionnaire`] flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionnaireItem {
    name: String,
    title: String,
    description: Option<String>,
    choices: Vec<QuestionnaireChoice>,
    input: Option<String>,
    multiple: bool,
    required: bool,
}

/// Create an item keyed `name` asking `title`.
///
/// `name` is the item's stable identity — it names nothing this widget paints,
/// and exists so an app can key its own answer storage off the same string the
/// source's `FormData` would.
pub fn questionnaire_item(name: impl Into<String>, title: impl Into<String>) -> QuestionnaireItem {
    QuestionnaireItem {
        name: name.into(),
        title: title.into(),
        description: None,
        choices: Vec::new(),
        input: None,
        multiple: false,
        required: false,
    }
}

impl QuestionnaireItem {
    /// Set the muted line under the prompt.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set this item's choices.
    pub fn choices(mut self, choices: Vec<QuestionnaireChoice>) -> Self {
        self.choices = choices;
        self
    }

    /// Add a free-text answer under the choices, showing `placeholder` while
    /// empty. The placeholder doubles as the field's accessible name.
    pub fn input(mut self, placeholder: impl Into<String>) -> Self {
        self.input = Some(placeholder.into());
        self
    }

    /// Let more than one choice be selected at once: the indicators become
    /// checkboxes rather than radios, and an answer accumulates instead of
    /// replacing.
    pub fn multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    /// Require an answer: Skip disappears, so the only way forward is to answer
    /// (see the [module docs](self) on what `required` does *not* change).
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// This item's stable name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The payload [`QuestionnaireView::on_answer`] reports: which item, and the
/// answer the interaction asks for it to hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionnaireAnswerEvent {
    /// The item's index into the list passed to [`questionnaire`].
    pub item: usize,
    /// The requested replacement answer.
    pub answer: QuestionnaireAnswer,
}

// ---- Action buttons --------------------------------------------------------

/// One button in the action row, in the order the source's grid places them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavButton {
    /// `col-start-1 justify-self-start`, `variant="outline"`.
    Previous,
    /// `col-start-2 justify-self-end`, `variant="outline"`.
    Skip,
    /// `col-start-3 justify-self-end`, `variant="default"`.
    Next,
    /// `col-start-3 justify-self-end`, `variant="default"` — shares Next's
    /// column, and the two are never visible together.
    Submit,
}

impl NavButton {
    /// Every button, in grid order.
    const ALL: [NavButton; 4] = [
        NavButton::Previous,
        NavButton::Skip,
        NavButton::Next,
        NavButton::Submit,
    ];

    /// This button's slot in the widget's fixed-length label/rect arrays.
    fn index(self) -> usize {
        self as usize
    }

    /// The source's default child text.
    fn label(self) -> &'static str {
        match self {
            NavButton::Previous => "Previous",
            NavButton::Skip => "Skip",
            NavButton::Next => "Next",
            NavButton::Submit => "Submit",
        }
    }

    /// Whether this button takes the solid `default` variant rather than
    /// `outline`.
    fn solid(self) -> bool {
        matches!(self, NavButton::Next | NavButton::Submit)
    }
}

/// A pointer target inside the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuestionnaireTarget {
    /// The `n`-th choice of the current item.
    Choice(usize),
    /// One action button.
    Nav(NavButton),
}

// ---- Palette ---------------------------------------------------------------

/// The resolved shadcn tokens this component paints from.
struct QuestionnaireColors {
    /// `--background`.
    background: Color,
    /// `--muted`, the checked choice fill and (at 50%) the hover fill.
    muted: Color,
    /// `--muted-foreground`, the progress line and the badge ink.
    muted_foreground: Color,
    /// `--input`, every resting border.
    input: Color,
    /// `--border`, the outline buttons' border.
    border: Color,
    /// `--primary` and `--primary-foreground`.
    primary: Color,
    on_primary: Color,
    /// `--accent` and `--accent-foreground`, an outline button's hover pair.
    accent: Color,
    on_accent: Color,
    /// `--foreground`, an outline button's resting ink.
    foreground: Color,
    /// `--destructive`, the error line and an invalid choice's border.
    destructive: Color,
    /// Whether the `dark:` variants apply.
    dark: bool,
}

/// Resolve the palette, falling back to the `neutral` preset's light table with
/// no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> QuestionnaireColors {
    let fallback: ColorScheme;
    let scheme = match theme {
        Some(theme) => theme.scheme(),
        None => {
            fallback = color_scheme_light();
            &fallback
        }
    };
    QuestionnaireColors {
        background: scheme.surface,
        muted: scheme.surface_container_highest,
        muted_foreground: scheme.on_surface_variant,
        input: scheme.outline_variant,
        border: scheme.outline,
        primary: scheme.primary,
        on_primary: scheme.on_primary,
        accent: scheme.primary_container,
        on_accent: scheme.on_primary_container,
        foreground: scheme.on_surface,
        destructive: scheme.error,
        dark: style::is_dark(theme),
    }
}

/// The lucide `CheckIcon` path (`M20 6 9 17l-5-5`), scaled from its 24-unit
/// viewBox to a `size`-square box and translated to `origin`.
fn check_path(origin: Point, size: f64) -> BezPath {
    let s = size / ICON_VIEWBOX;
    let at = |x: f64, y: f64| Point::new(origin.x + x * s, origin.y + y * s);
    let mut path = BezPath::new();
    path.move_to(at(4.0, 12.0));
    path.line_to(at(9.0, 17.0));
    path.line_to(at(20.0, 6.0));
    path
}

/// The progress line's style (`text-xs font-medium`) in the live theme's
/// `LabelMedium` family.
fn progress_style(theme: Option<&Theme>) -> TextStyle {
    themed_family(
        TextStyle {
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(style::TEXT_XS as f32, SHAPING_INK)
        },
        theme,
        ThemeTextType::LabelMedium,
    )
}

/// An action button's label style (`text-sm font-medium`, `buttonVariants`) in
/// the live theme's `LabelLarge` family.
fn nav_style(theme: Option<&Theme>) -> TextStyle {
    themed_family(
        TextStyle {
            weight: FontWeight::MEDIUM,
            ..TextStyle::new(style::TEXT_SM as f32, SHAPING_INK)
        },
        theme,
        ThemeTextType::LabelLarge,
    )
}

/// The error line's style (`text-sm`) in the live theme's `BodyMedium` family.
fn error_style(theme: Option<&Theme>) -> TextStyle {
    themed_family(
        TextStyle::new(style::TEXT_SM as f32, SHAPING_INK),
        theme,
        ThemeTextType::BodyMedium,
    )
}

/// A shortcut badge's style (`font-mono text-[0.625rem] font-medium`).
fn shortcut_style() -> TextStyle {
    TextStyle {
        // Explicit, not themed: `TypeScale` has no monospace role to resolve from.
        family: mono_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(SHORTCUT_TEXT as f32, SHAPING_INK)
    }
}

// ---- View ------------------------------------------------------------------

/// A view-held, typed answer callback (erased on build).
type OnAnswer<State> = Rc<dyn Fn(&mut State, QuestionnaireAnswerEvent)>;
/// A view-held, typed navigation callback (erased on build).
type OnNavigate<State> = Rc<dyn Fn(&mut State, usize)>;
/// A view-held, typed submit callback (erased on build).
type OnSubmit<State> = Rc<dyn Fn(&mut State)>;

/// A declarative shadcn questionnaire. See the [module docs](self).
pub struct QuestionnaireView<State: 'static> {
    items: Vec<QuestionnaireItem>,
    answers: Vec<QuestionnaireAnswer>,
    current: usize,
    shortcuts: QuestionnaireShortcuts,
    on_answer: OnAnswer<State>,
    on_navigate: OnNavigate<State>,
    on_submit: OnSubmit<State>,
}

/// Create a questionnaire showing `items[current]`, reflecting `answers`.
///
/// A **controlled** component: `current` and `answers` are the app's, and the
/// three reporting seams — [`on_answer`](QuestionnaireView::on_answer),
/// [`on_navigate`](QuestionnaireView::on_navigate),
/// [`on_submit`](QuestionnaireView::on_submit) — each default to a no-op, so a
/// flow wires the ones it acts on. `answers` shorter than `items` reads as
/// unanswered from there on.
pub fn questionnaire<State: 'static>(
    items: Vec<QuestionnaireItem>,
    current: usize,
    answers: Vec<QuestionnaireAnswer>,
) -> QuestionnaireView<State> {
    QuestionnaireView {
        items,
        answers,
        current,
        shortcuts: QuestionnaireShortcuts::default(),
        on_answer: Rc::new(|_, _| {}),
        on_navigate: Rc::new(|_, _| {}),
        on_submit: Rc::new(|_| {}),
    }
}

impl<State: 'static> QuestionnaireView<State> {
    /// Label the choices with keyboard shortcuts (default
    /// [`QuestionnaireShortcuts::None`]).
    pub fn shortcuts(mut self, shortcuts: QuestionnaireShortcuts) -> Self {
        self.shortcuts = shortcuts;
        self
    }

    /// Report a requested answer for one item — a choice press, a shortcut key,
    /// an edit of the free-text field, or a Skip.
    pub fn on_answer<F: Fn(&mut State, QuestionnaireAnswerEvent) + 'static>(
        mut self,
        on_answer: F,
    ) -> Self {
        self.on_answer = Rc::new(on_answer);
        self
    }

    /// Report a requested item index — Previous, Next, Skip's advance, or an
    /// arrow key.
    pub fn on_navigate<F: Fn(&mut State, usize) + 'static>(mut self, on_navigate: F) -> Self {
        self.on_navigate = Rc::new(on_navigate);
        self
    }

    /// Report the flow finishing: Submit (or Skip/Enter on the last item) after
    /// the current item validates.
    pub fn on_submit<F: Fn(&mut State) + 'static>(mut self, on_submit: F) -> Self {
        self.on_submit = Rc::new(on_submit);
        self
    }

    /// The current item, if `current` is in range.
    fn item(&self) -> Option<&QuestionnaireItem> {
        self.items.get(self.current)
    }

    /// The current item's answer (an unanswered default when the list is short).
    fn answer(&self) -> QuestionnaireAnswer {
        self.answers.get(self.current).cloned().unwrap_or_default()
    }

    /// The pods for the current item, in the fixed order
    /// `[title, description?, choice…, input?]`.
    fn content_views(&self) -> Vec<AnyView<State>> {
        let Some(item) = self.item() else {
            return Vec::new();
        };
        let mut views = vec![any(text(item.title.clone())
            .size(style::TEXT_BASE as f32)
            .weight(FontWeight::MEDIUM)
            .themed_family(ThemeTextType::TitleMedium)
            .themed_role(ThemeTextColor::OnSurface))];
        if let Some(description) = &item.description {
            views.push(any(text(description.clone())
                .size(style::TEXT_SM as f32)
                .themed_family(ThemeTextType::BodyMedium)
                .themed_role(ThemeTextColor::OnSurfaceVariant)));
        }
        for choice in &item.choices {
            views.push(choice_content(choice));
        }
        if let Some(placeholder) = &item.input {
            views.push(self.input_view(item, placeholder));
        }
        views
    }

    /// The free-text field, with its own chrome suppressed so this widget paints
    /// the `.cn-questionnaire-input` recipe around it.
    ///
    /// The recipe is ported here rather than reached for through
    /// [`crate::input`]: the questionnaire's field is `h-8 rounded-lg px-2.5`
    /// where the standalone control is `h-9 rounded-md px-3`, so only the wrapped
    /// baseline field is shared, not the chrome.
    ///
    /// Its typed text keeps the system UI family: `text_input` takes a style
    /// only at build and has no theme-resolved family to opt into.
    // erasure: keep pushed into a heterogeneous Vec<AnyView> row list
    fn input_view(&self, item: &QuestionnaireItem, placeholder: &str) -> AnyView<State> {
        let on_answer = self.on_answer.clone();
        let index = self.current;
        let answer = self.answer();
        let multiple = item.multiple;
        any(text_input(
            answer.text.clone(),
            move |state: &mut State, typed: String| {
                let mut next = answer.clone();
                next.skipped = false;
                // Single-select: the two answer routes are one value here, so
                // typing releases the choice (see the module docs).
                if !multiple && !typed.trim().is_empty() {
                    next.values.clear();
                }
                next.text = typed;
                (on_answer)(
                    state,
                    QuestionnaireAnswerEvent {
                        item: index,
                        answer: next,
                    },
                );
            },
        )
        .placeholder(placeholder.to_string())
        .padding(INPUT_PAD_X, INPUT_PAD_Y)
        .border_width(0.0)
        .focus_ring_width(0.0)
        .corner_radius(ShadcnRadius::shadcn().lg))
    }
}

/// One choice's content column: the label, plus the muted description line at
/// `gap-0.5`.
fn choice_content<State: 'static>(choice: &QuestionnaireChoice) -> AnyView<State> {
    let label = text(choice.label.clone())
        .size(style::TEXT_SM as f32)
        .weight(FontWeight::MEDIUM)
        .themed_family(ThemeTextType::LabelLarge)
        .themed_role(ThemeTextColor::OnSurface);
    let Some(description) = &choice.description else {
        return any(label);
    };
    any(column()
        .child(label)
        .child(SizedBox(None, Some(style::spacing(0.5))))
        .child(
            text(description.clone())
                .size(style::TEXT_SM as f32)
                .themed_family(ThemeTextType::BodyMedium)
                .themed_role(ThemeTextColor::OnSurfaceVariant),
        )
        .cross_axis(CrossAxisAlignment::Start))
}

/// The badge label for each choice of `item`, `None` where the mode assigns
/// none — disabled choices are skipped before keys are handed out
/// (`getShortcutByChoiceValue`).
fn shortcut_labels(
    item: Option<&QuestionnaireItem>,
    shortcuts: QuestionnaireShortcuts,
) -> Vec<Option<String>> {
    let Some(item) = item else {
        return Vec::new();
    };
    let mut next = 0usize;
    item.choices
        .iter()
        .map(|choice| {
            if choice.disabled {
                return None;
            }
            let label = shortcuts.label(next);
            next += 1;
            label
        })
        .collect()
}

impl<State: 'static> View<State> for QuestionnaireView<State> {
    type Element = QuestionnaireWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> QuestionnaireWidget {
        let views = self.content_views();
        let mut widget = QuestionnaireWidget {
            items: self.items.clone(),
            answers: self.answers.clone(),
            current: self.current,
            shortcuts: self.shortcuts,
            pods: views.iter().map(|view| build_child(view, ctx)).collect(),
            has_description: false,
            has_input: false,
            shortcut_labels: Vec::new(),
            shortcut_badges: Vec::new(),
            choice_rects: Vec::new(),
            input_rect: None,
            nav_rects: [None; 4],
            progress: LabelRun::new(String::new()),
            progress_origin: Point::ORIGIN,
            error: LabelRun::new(String::new()),
            error_origin: None,
            nav_labels: NavButton::ALL.map(|button| LabelRun::new(button.label())),
            laid_out_invalid: false,
            validation_attempted: false,
            focused_choice: 0,
            hovered: None,
            captured: None,
            on_answer: erase_callback_arg(&self.on_answer),
            on_navigate: erase_callback_arg(&self.on_navigate),
            on_submit: erase_callback(&self.on_submit),
        };
        widget.sync_item();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut QuestionnaireWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so every adapter is reinstalled each pass.
        element.on_answer = erase_callback_arg(&self.on_answer);
        element.on_navigate = erase_callback_arg(&self.on_navigate);
        element.on_submit = erase_callback(&self.on_submit);

        let prev_views = prev.content_views();
        let next_views = self.content_views();
        let prev_refs: Vec<&AnyView<State>> = prev_views.iter().collect();
        let next_refs: Vec<&AnyView<State>> = next_views.iter().collect();
        let mut flags = rebuild_children(
            &prev_refs,
            &next_refs,
            &mut element.pods,
            ctx,
            |view: &&AnyView<State>| *view,
            |_| None,
        );

        let moved = prev.current != self.current || prev.items != self.items;
        if moved || prev.shortcuts != self.shortcuts {
            // The app is the source of truth: adopt the confirmed item.
            element.items = self.items.clone();
            element.current = self.current;
            element.shortcuts = self.shortcuts;
            element.sync_item();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.answers != self.answers {
            element.answers = self.answers.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut QuestionnaireWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.content_views().iter().zip(element.pods.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

// ---- Widget ----------------------------------------------------------------

/// The retained widget for a [`QuestionnaireView`].
pub struct QuestionnaireWidget {
    items: Vec<QuestionnaireItem>,
    /// The app-confirmed answers (source of truth; adopted on `rebuild`).
    answers: Vec<QuestionnaireAnswer>,
    /// The app-confirmed item index.
    current: usize,
    shortcuts: QuestionnaireShortcuts,
    /// `[title, description?, choice…, input?]` — one flat list so
    /// [`visit_children`] publishes them all.
    pods: Vec<ChildPod>,
    has_description: bool,
    has_input: bool,
    /// Per-choice badge label, parallel to the current item's choices.
    shortcut_labels: Vec<Option<String>>,
    shortcut_badges: Vec<LabelRun>,
    /// Widget-local boxes resolved by `layout`, and the only thing hit testing
    /// reads.
    choice_rects: Vec<Rect>,
    input_rect: Option<Rect>,
    nav_rects: [Option<Rect>; 4],
    progress: LabelRun,
    progress_origin: Point,
    error: LabelRun,
    /// `Some` only while the error line is showing (it is `hidden` otherwise, so
    /// it takes no space).
    error_origin: Option<Point>,
    nav_labels: [LabelRun; 4],
    /// The invalid state the last `layout` reserved room for — a mismatch at
    /// paint time is what asks for the relayout the error line needs.
    laid_out_invalid: bool,
    /// The source's `validationAttempted`: a blocked Next/Submit latches the
    /// error line until the item changes.
    validation_attempted: bool,
    /// The choice the roving focus sits on.
    focused_choice: usize,
    /// The target under the pointer, latched for hover chrome and self-corrected
    /// from `PaintCtx::is_hovered`.
    hovered: Option<QuestionnaireTarget>,
    /// The target a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<QuestionnaireTarget>,
    on_answer: ErasedArgCallback<QuestionnaireAnswerEvent>,
    on_navigate: ErasedArgCallback<usize>,
    on_submit: ErasedCallback,
}

impl QuestionnaireWidget {
    /// Re-derive everything keyed to the current item, and drop the transient
    /// state that belonged to the previous one.
    fn sync_item(&mut self) {
        let item = self.items.get(self.current);
        self.has_description = item.is_some_and(|i| i.description.is_some());
        self.has_input = item.is_some_and(|i| i.input.is_some());
        self.shortcut_labels = shortcut_labels(item, self.shortcuts);
        self.shortcut_badges = self
            .shortcut_labels
            .iter()
            .map(|label| LabelRun::new(label.clone().unwrap_or_default()))
            .collect();
        self.choice_rects.clear();
        self.input_rect = None;
        self.validation_attempted = false;
        self.focused_choice = 0;
        self.captured = None;
        self.hovered = None;
    }

    /// The current item, if `current` is in range.
    fn item(&self) -> Option<&QuestionnaireItem> {
        self.items.get(self.current)
    }

    /// The current item's answer (an unanswered default when the list is short).
    fn answer(&self) -> QuestionnaireAnswer {
        self.answers.get(self.current).cloned().unwrap_or_default()
    }

    /// The current item's status.
    fn status(&self) -> QuestionnaireStatus {
        self.answer().status()
    }

    /// `Question x of n` (`Questionnaire.Progress`'s `aria-valuetext`).
    fn progress_text(&self) -> String {
        format!("Question {} of {}", self.current + 1, self.items.len())
    }

    /// Whether the current item validates: intentionally skipped, or answered
    /// (`use-questionnaire-item.ts`'s `valid`).
    fn is_valid(&self) -> bool {
        let required = self.item().is_some_and(|item| item.required);
        match self.status() {
            QuestionnaireStatus::Skipped => !required,
            QuestionnaireStatus::Answered => true,
            QuestionnaireStatus::Unanswered => false,
        }
    }

    /// Whether the error line shows: a latched validation attempt over an item
    /// that still does not validate, and that was not deliberately skipped.
    fn is_invalid(&self) -> bool {
        let required = self.item().is_some_and(|item| item.required);
        let intentionally_skipped = self.status() == QuestionnaireStatus::Skipped && !required;
        !intentionally_skipped && self.validation_attempted && !self.is_valid()
    }

    /// The error line's text (`Questionnaire.Error`'s default children).
    fn error_text(&self) -> &'static str {
        if self.item().is_some_and(|item| item.required) {
            "Choose an answer to continue."
        } else {
            "Choose an answer or skip this question."
        }
    }

    /// Whether the current item is the last one.
    fn is_last(&self) -> bool {
        !self.items.is_empty() && self.current + 1 == self.items.len()
    }

    /// Whether `button` is rendered, per the source's
    /// `QuestionnaireNavigationState.visible`.
    fn nav_visible(&self, button: NavButton) -> bool {
        let total = self.items.len();
        match button {
            NavButton::Previous => total > 1 && self.current > 0,
            NavButton::Skip => self.item().is_some_and(|item| !item.required),
            NavButton::Next => total > 1 && !self.is_last(),
            NavButton::Submit => total > 0 && self.is_last(),
        }
    }

    /// The current item's choices.
    fn choices(&self) -> &[QuestionnaireChoice] {
        self.item().map_or(&[], |item| item.choices.as_slice())
    }

    /// Whether choice `index` accepts interaction.
    fn choice_enabled(&self, index: usize) -> bool {
        self.choices().get(index).is_some_and(|c| !c.disabled)
    }

    /// Whether choice `index` is currently selected.
    fn choice_checked(&self, index: usize) -> bool {
        let answer = self.answer();
        self.choices()
            .get(index)
            .is_some_and(|choice| answer.values.iter().any(|v| v == &choice.value))
    }

    /// The target under widget-local `pos`, considering only live ones.
    fn target_at(&self, pos: Point) -> Option<QuestionnaireTarget> {
        for (index, rect) in self.choice_rects.iter().enumerate() {
            if rect.contains(pos) {
                return self
                    .choice_enabled(index)
                    .then_some(QuestionnaireTarget::Choice(index));
            }
        }
        NavButton::ALL
            .into_iter()
            .find(|button| self.nav_rects[button.index()].is_some_and(|rect| rect.contains(pos)))
            .map(QuestionnaireTarget::Nav)
    }

    /// The free-text field's pod, when the current item has one.
    fn input_pod(&mut self) -> Option<&mut ChildPod> {
        if self.has_input {
            self.pods.last_mut()
        } else {
            None
        }
    }

    /// Whether the free-text field holds focus — the read that keeps the
    /// shortcut and arrow keys out of a typing user's way (`isTextEntryTarget`).
    fn input_focused(&self) -> bool {
        self.has_input && self.pods.last().is_some_and(ChildPod::is_focused)
    }

    /// Report the answer choice `index` asks for, without touching `answers`.
    fn request_choice(&mut self, ctx: &mut EventCtx, index: usize) {
        if !self.choice_enabled(index) {
            return;
        }
        let Some(choice) = self.choices().get(index) else {
            return;
        };
        let value = choice.value.clone();
        let multiple = self.item().is_some_and(|item| item.multiple);
        let mut answer = self.answer();
        answer.skipped = false;
        if multiple {
            if let Some(at) = answer.values.iter().position(|v| v == &value) {
                answer.values.remove(at);
            } else {
                answer.values.push(value);
            }
        } else {
            answer.values = vec![value];
            // Single-select: the free text and the choice are one value here.
            answer.text.clear();
        }
        let item = self.current;
        (self.on_answer)(ctx, QuestionnaireAnswerEvent { item, answer });
    }

    /// Report the previous item (`goPrevious`, which never validates).
    fn go_previous(&mut self, ctx: &mut EventCtx) {
        if self.current == 0 {
            return;
        }
        let target = self.current - 1;
        (self.on_navigate)(ctx, target);
    }

    /// Validate, then report the next item — or the submit when the current item
    /// is the last (`confirmCurrent`).
    fn confirm(&mut self, ctx: &mut EventCtx) {
        if !self.is_valid() {
            self.validation_attempted = true;
            ctx.request_redraw();
            return;
        }
        if self.is_last() {
            (self.on_submit)(ctx);
        } else {
            let target = self.current + 1;
            (self.on_navigate)(ctx, target);
        }
    }

    /// `goNext`: `confirm` restricted to a forward move, so it no-ops on the last
    /// item rather than submitting.
    fn go_next(&mut self, ctx: &mut EventCtx) {
        if self.is_last() {
            return;
        }
        self.confirm(ctx);
    }

    /// `skipCurrent`: mark the item skipped, then advance (or submit on the last
    /// item). A required item refuses.
    fn skip(&mut self, ctx: &mut EventCtx) {
        if self.item().is_none_or(|item| item.required) {
            return;
        }
        let item = self.current;
        (self.on_answer)(
            ctx,
            QuestionnaireAnswerEvent {
                item,
                answer: QuestionnaireAnswer {
                    skipped: true,
                    ..QuestionnaireAnswer::default()
                },
            },
        );
        if self.is_last() {
            (self.on_submit)(ctx);
        } else {
            (self.on_navigate)(ctx, item + 1);
        }
    }

    /// Run the target a press released on.
    fn activate(&mut self, ctx: &mut EventCtx, target: QuestionnaireTarget) {
        match target {
            QuestionnaireTarget::Choice(index) => self.request_choice(ctx, index),
            QuestionnaireTarget::Nav(NavButton::Previous) => self.go_previous(ctx),
            QuestionnaireTarget::Nav(NavButton::Skip) => self.skip(ctx),
            QuestionnaireTarget::Nav(NavButton::Next | NavButton::Submit) => self.confirm(ctx),
        }
    }

    /// The next enabled choice `step` places from the roving focus, wrapping, or
    /// `None` when no choice is enabled.
    fn step_choice(&self, step: isize) -> Option<usize> {
        let len = self.choices().len();
        if len == 0 {
            return None;
        }
        let mut index = self.focused_choice;
        for _ in 0..len {
            index = ((index as isize + step).rem_euclid(len as isize)) as usize;
            if self.choice_enabled(index) {
                return Some(index);
            }
        }
        None
    }

    /// The choice whose badge is `label`.
    fn choice_for_shortcut(&self, label: &str) -> Option<usize> {
        self.shortcut_labels
            .iter()
            .position(|assigned| assigned.as_deref() == Some(label))
    }

    /// The `Widget::event` key arm.
    ///
    /// Enter confirms from anywhere — including from inside the free-text field,
    /// which is what the source does (`handleKeyDown`'s `Enter` branch confirms
    /// on a filled answer). Everything else defers to the field while it is
    /// focused, so a shortcut letter types rather than selecting.
    fn handle_key(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        key: &KeyEvent,
    ) -> EventResult {
        if let Key::Named(NamedKey::Enter) = &key.key {
            self.confirm(ctx);
            return EventResult::Handled;
        }
        if self.input_focused() {
            return self.route_to_input(ctx, event);
        }
        match &key.key {
            Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp) => {
                let step = if matches!(key.key, Key::Named(NamedKey::ArrowDown)) {
                    1
                } else {
                    -1
                };
                let Some(next) = self.step_choice(step) else {
                    return EventResult::Ignored;
                };
                self.focused_choice = next;
                ctx.request_redraw();
                // A radio group selects as it moves (`moveAnswerFocus` clicks the
                // radio it lands on); a multi-select item only moves.
                if self.item().is_some_and(|item| !item.multiple) {
                    self.request_choice(ctx, next);
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.go_previous(ctx);
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowRight) => {
                // The source gates the forward arrow on a touched item, so an
                // untouched one is stepped past only through Next.
                if self.status() != QuestionnaireStatus::Unanswered {
                    self.go_next(ctx);
                }
                EventResult::Handled
            }
            Key::Character(typed) if typed == " " => {
                let focused = self.focused_choice;
                if !self.choice_enabled(focused) {
                    return EventResult::Ignored;
                }
                self.request_choice(ctx, focused);
                EventResult::Handled
            }
            Key::Character(typed) => {
                let Some(label) = self.shortcuts.shortcut_for_key(typed) else {
                    return EventResult::Ignored;
                };
                let Some(index) = self.choice_for_shortcut(&label) else {
                    return EventResult::Ignored;
                };
                self.focused_choice = index;
                ctx.request_redraw();
                self.request_choice(ctx, index);
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    /// Forward an event to the free-text field, if there is one.
    fn route_to_input(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match self.input_pod() {
            Some(pod) => route_event_single(pod, ctx, event),
            None => EventResult::Ignored,
        }
    }

    /// The `Widget::event` pointer arm: the field owns its own row, everything
    /// else is this widget's press contract.
    fn handle_pointer(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
    ) -> EventResult {
        let over_input = self
            .input_rect
            .is_some_and(|rect| rect.contains(p.position));
        let field_active = self.pods.last().is_some_and(ChildPod::is_active);
        if self.has_input && self.captured.is_none() && (over_input || field_active) {
            let result = self.route_to_input(ctx, event);
            if matches!(p.phase, PointerPhase::Move) {
                // Asked for after routing so this widget's shape wins (the
                // baseline field asks for none of its own).
                ctx.set_cursor(CursorIcon::Text);
            }
            return result;
        }
        match p.phase {
            PointerPhase::Move => {
                if self.captured.is_some() {
                    // Captured: re-ask so the shape survives a drag outside the
                    // armed target.
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let target = self.target_at(p.position);
                // Claimed after the field routing above (the claim-ordering rule).
                if target.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != target {
                    self.hovered = target;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(target) = self.target_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.captured = Some(target);
                if let QuestionnaireTarget::Choice(index) = target {
                    self.focused_choice = index;
                }
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(target) = self.captured.take() else {
                    return EventResult::Ignored;
                };
                if self.target_at(p.position) == Some(target) {
                    self.activate(ctx, target);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.take().is_none() {
                    return EventResult::Ignored;
                }
                // Internal flags only — never state, never a callback.
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    /// Paint one choice row's box, indicator and badge (everything but its
    /// content pod, which paints on top).
    fn paint_choice(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        index: usize,
        chrome: &ChoiceChrome,
    ) {
        let Some(rect) = self.choice_rects.get(index) else {
            return;
        };
        let enabled = self.choice_enabled(index);
        let tint = |color: Color| style::disabled_tint(color, !enabled);
        let checked = self.choice_checked(index);
        let hovered = self.hovered == Some(QuestionnaireTarget::Choice(index));
        let focused = chrome.focused && index == self.focused_choice;
        let box_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
        let box_size = Size::new(rect.width(), rect.height());

        // `bg-transparent` at rest; `data-checked:bg-muted` outranks
        // `hover:bg-muted/50`, and `dark:bg-input/20` is the resting dark wash.
        let fill = if checked {
            Some(chrome.colors.muted)
        } else if hovered {
            Some(style::with_alpha(chrome.colors.muted, CHOICE_HOVER_ALPHA))
        } else if chrome.colors.dark {
            Some(style::scale_alpha(
                chrome.colors.input,
                CHOICE_DARK_FILL_ALPHA,
            ))
        } else {
            None
        };
        if let Some(fill) = fill {
            scene.fill_rounded_rect(box_origin, box_size, chrome.radius_lg, tint(fill));
        }

        let resting_border = if chrome.invalid {
            chrome.colors.destructive
        } else if checked {
            style::scale_alpha(chrome.colors.primary, CHOICE_CHECKED_BORDER_ALPHA)
        } else {
            chrome.colors.input
        };
        let border = if focused { chrome.ring } else { resting_border };
        stroke_border(scene, box_origin, box_size, chrome.radius_lg, tint(border));

        // The indicator: `size-4`, a circle for a radio and `rounded-[4px]` for a
        // checkbox, nudged onto the label's first line.
        let indicator_origin = Point::new(
            box_origin.x + CHOICE_PAD_X,
            box_origin.y + CHOICE_PAD_Y + INDICATOR_NUDGE,
        );
        let indicator_size = Size::new(INDICATOR_SIZE, INDICATOR_SIZE);
        let indicator_radius = if chrome.multiple {
            INDICATOR_RADIUS
        } else {
            INDICATOR_SIZE / 2.0
        };
        let indicator_fill = if checked {
            Some(chrome.colors.primary)
        } else if chrome.colors.dark {
            Some(style::scale_alpha(chrome.colors.input, DARK_FILL_ALPHA))
        } else {
            None
        };
        if let Some(fill) = indicator_fill {
            scene.fill_rounded_rect(
                indicator_origin,
                indicator_size,
                indicator_radius,
                tint(fill),
            );
        }
        let indicator_border = if checked {
            chrome.colors.primary
        } else {
            chrome.colors.input
        };
        stroke_border(
            scene,
            indicator_origin,
            indicator_size,
            indicator_radius,
            tint(indicator_border),
        );
        if checked {
            if chrome.multiple {
                let inset = (INDICATOR_SIZE - INDICATOR_CHECK_SIZE) / 2.0;
                scene.stroke_path(
                    indicator_origin,
                    &check_path(Point::new(inset, inset), INDICATOR_CHECK_SIZE),
                    CHECK_STROKE_VIEWBOX * INDICATOR_CHECK_SIZE / ICON_VIEWBOX,
                    &Brush::Solid(tint(chrome.colors.on_primary)),
                );
            } else {
                let inset = (INDICATOR_SIZE - INDICATOR_DOT_SIZE) / 2.0;
                scene.fill_rounded_rect(
                    Point::new(indicator_origin.x + inset, indicator_origin.y + inset),
                    Size::new(INDICATOR_DOT_SIZE, INDICATOR_DOT_SIZE),
                    INDICATOR_DOT_SIZE / 2.0,
                    tint(chrome.colors.on_primary),
                );
            }
        }

        // The badge: `ms-auto`, so it rides the trailing edge.
        if self.shortcut_labels.get(index).is_some_and(Option::is_some) {
            let badge_origin = Point::new(
                box_origin.x + box_size.width - CHOICE_PAD_X - QUESTIONNAIRE_SHORTCUT_SIZE,
                box_origin.y + CHOICE_PAD_Y + INDICATOR_NUDGE,
            );
            let badge_size = Size::new(QUESTIONNAIRE_SHORTCUT_SIZE, QUESTIONNAIRE_SHORTCUT_SIZE);
            scene.fill_rounded_rect(
                badge_origin,
                badge_size,
                chrome.radius_md,
                tint(chrome.colors.background),
            );
            stroke_border(
                scene,
                badge_origin,
                badge_size,
                chrome.radius_md,
                tint(chrome.colors.input),
            );
            if let Some(badge) = self.shortcut_badges.get(index) {
                let text_size = badge.size();
                badge.paint(
                    Point::new(
                        badge_origin.x + (badge_size.width - text_size.width) / 2.0,
                        badge_origin.y + (badge_size.height - text_size.height) / 2.0,
                    ),
                    tint(chrome.colors.muted_foreground),
                    scene,
                );
            }
        }

        if focused {
            style::draw_focus_ring(scene, box_origin, box_size, chrome.radius_lg, chrome.ring);
        }
    }

    /// Paint the four action buttons over their resolved rects.
    fn paint_actions(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        colors: &QuestionnaireColors,
        radius: f64,
        theme: Option<&Theme>,
    ) {
        for button in NavButton::ALL {
            let Some(rect) = self.nav_rects[button.index()] else {
                continue;
            };
            let hovered = self.hovered == Some(QuestionnaireTarget::Nav(button));
            let pressed = self.captured == Some(QuestionnaireTarget::Nav(button));
            let active = hovered || pressed;
            let box_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let box_size = Size::new(rect.width(), rect.height());

            let (fill, ink) = match (button.solid(), active) {
                // `bg-primary hover:bg-primary/90`.
                (true, false) => (colors.primary, colors.on_primary),
                (true, true) => (
                    style::with_alpha(colors.primary, style::HOVER_SOLID_ALPHA),
                    colors.on_primary,
                ),
                // `bg-background hover:bg-accent hover:text-accent-foreground`.
                (false, false) => (colors.background, colors.foreground),
                (false, true) => (colors.accent, colors.on_accent),
            };
            if !button.solid() {
                style::draw_shadow(scene, box_origin, box_size, radius, style::SHADOW_XS, theme);
            }
            scene.fill_rounded_rect(box_origin, box_size, radius, fill);
            if !button.solid() {
                stroke_border(scene, box_origin, box_size, radius, colors.border);
            }
            let label = &self.nav_labels[button.index()];
            let text_size = label.size();
            label.paint(
                Point::new(
                    box_origin.x + (box_size.width - text_size.width) / 2.0,
                    box_origin.y + (box_size.height - text_size.height) / 2.0,
                ),
                ink,
                scene,
            );
        }
    }
}

/// The per-pass values every choice row paints from, resolved once.
struct ChoiceChrome {
    colors: QuestionnaireColors,
    /// `rounded-lg` — the choice box and the free-text field.
    radius_lg: f64,
    /// `rounded-md` — the shortcut badge and the action buttons.
    radius_md: f64,
    ring: Color,
    /// Whether the widget holds focus and the field does not (so the roving ring
    /// belongs to a choice).
    focused: bool,
    /// Whether the item is in its `data-invalid` state.
    invalid: bool,
    /// Whether the item selects more than one choice.
    multiple: bool,
}

/// Stroke a 1px border inside `origin`/`size`, the way every bordered control in
/// this catalog does (the stroke rides half a width outward so the painted band
/// lands on the box's own edge).
fn stroke_border(scene: &mut dyn PaintScene, origin: Point, size: Size, radius: f64, color: Color) {
    let inset = style::BORDER_WIDTH / 2.0;
    let outline = RoundedRect::from_rect(
        Rect::from_origin_size(Point::ORIGIN, size).inset(-inset),
        radius + inset,
    );
    scene.stroke_path(
        origin,
        &Shape::to_path(&outline, PATH_TOLERANCE),
        style::BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

impl Widget for QuestionnaireWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };
        let full = |w: f64| BoxConstraints::new(Size::new(w, 0.0), Size::new(w, f64::INFINITY));
        let mut y = 0.0;

        // The progress line.
        let progress_text = self.progress_text();
        self.progress.set_content(progress_text);
        let progress_style = progress_style(Theme::from_layout_ctx(ctx));
        let progress_size = self.progress.layout(ctx, &progress_style);
        self.progress_origin = Point::ORIGIN;
        y += progress_size.height + QUESTIONNAIRE_GAP;

        // The action labels, measured up front so the row can be placed last.
        let mut nav_widths = [0.0f64; 4];
        let nav_style = nav_style(Theme::from_layout_ctx(ctx));
        for button in NavButton::ALL {
            let size = self.nav_labels[button.index()].layout(ctx, &nav_style);
            nav_widths[button.index()] = size.width + 2.0 * BUTTON_PAD_X;
        }

        // The prompt.
        let mut index = 0usize;
        if let Some(pod) = self.pods.get_mut(index) {
            let size = pod.layout_child(ctx, &full(width));
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
            index += 1;
        }
        if self.has_description {
            y += QUESTIONNAIRE_GAP;
            if let Some(pod) = self.pods.get_mut(index) {
                let size = pod.layout_child(ctx, &full(width));
                pod.set_origin(Point::new(0.0, y));
                y += size.height;
                index += 1;
            }
        } else {
            // `mb-4` on a title with no description sibling.
            y += TITLE_MARGIN_BOTTOM;
        }
        y += QUESTIONNAIRE_GAP;

        // The choices.
        self.choice_rects.clear();
        let choice_count = self.choices().len();
        for choice in 0..choice_count {
            let badged = self
                .shortcut_labels
                .get(choice)
                .is_some_and(Option::is_some);
            if badged && let Some(badge) = self.shortcut_badges.get_mut(choice) {
                // Measured here so `paint` can centre the glyph in the badge box.
                badge.layout(ctx, &shortcut_style());
            }
            let lead = CHOICE_PAD_X + INDICATOR_SIZE + CHOICE_GAP;
            let trail = CHOICE_PAD_X
                + if badged {
                    QUESTIONNAIRE_SHORTCUT_SIZE + CHOICE_GAP
                } else {
                    0.0
                };
            let content_width = (width - lead - trail).max(0.0);
            let mut content = Size::ZERO;
            if let Some(pod) = self.pods.get_mut(index) {
                content = pod.layout_child(ctx, &full(content_width));
                pod.set_origin(Point::new(lead, y + CHOICE_PAD_Y));
            }
            let height = (content.height + 2.0 * CHOICE_PAD_Y).max(QUESTIONNAIRE_CHOICE_MIN_HEIGHT);
            self.choice_rects.push(Rect::new(0.0, y, width, y + height));
            y += height + CHOICES_GAP;
            index += 1;
        }

        // The free-text field.
        self.input_rect = None;
        if self.has_input {
            if let Some(pod) = self.pods.get_mut(index) {
                pod.layout_child(
                    ctx,
                    &BoxConstraints::tight(Size::new(width, QUESTIONNAIRE_INPUT_HEIGHT)),
                );
                pod.set_origin(Point::new(0.0, y));
            }
            self.input_rect = Some(Rect::new(0.0, y, width, y + QUESTIONNAIRE_INPUT_HEIGHT));
            y += QUESTIONNAIRE_INPUT_HEIGHT + CHOICES_GAP;
        }
        if choice_count > 0 || self.has_input {
            y -= CHOICES_GAP;
        }

        // The error line, which takes no space while hidden.
        self.laid_out_invalid = self.is_invalid();
        self.error_origin = None;
        if self.laid_out_invalid {
            self.error.set_content(self.error_text());
            y += ERROR_MARGIN_TOP;
            let error_style = error_style(Theme::from_layout_ctx(ctx));
            let size = self.error.layout(ctx, &error_style);
            self.error_origin = Some(Point::new(0.0, y));
            y += size.height;
        }
        y += QUESTIONNAIRE_GAP;

        // The action row: Previous leads, Next-or-Submit anchors the trailing
        // edge, Skip sits inside it.
        let row_top = y;
        self.nav_rects = [None; 4];
        let mut trailing = width;
        for button in [NavButton::Next, NavButton::Submit, NavButton::Skip] {
            if !self.nav_visible(button) {
                continue;
            }
            let button_width = nav_widths[button.index()];
            self.nav_rects[button.index()] = Some(Rect::new(
                trailing - button_width,
                row_top,
                trailing,
                row_top + BUTTON_HEIGHT,
            ));
            trailing -= button_width + ACTIONS_GAP;
        }
        if self.nav_visible(NavButton::Previous) {
            let button_width = nav_widths[NavButton::Previous.index()];
            self.nav_rects[NavButton::Previous.index()] = Some(Rect::new(
                0.0,
                row_top,
                button_width,
                row_top + BUTTON_HEIGHT,
            ));
        }
        y = row_top + BUTTON_HEIGHT;

        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read: a pointer that left this widget sends it
        // nothing, so the latched target is cleared from the path membership.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let invalid = self.is_invalid();
        if invalid != self.laid_out_invalid {
            // The error line changes the widget's height, so it needs a relayout
            // rather than a bare repaint.
            ctx.request_layout();
        }
        let origin = ctx.origin();
        let focused = ctx.has_focus() && !self.input_focused();
        let multiple = self.item().is_some_and(|item| item.multiple);

        // Phase 1: everything that reads the theme and sits *under* the pods.
        let chrome = {
            let theme = Theme::from_paint_ctx(ctx);
            let radii = ShadcnTokens::resolve_radius(None, theme);
            ChoiceChrome {
                colors: resolve_colors(theme),
                radius_lg: radii.lg,
                radius_md: radii.md,
                ring: style::ring_color(None, theme),
                focused,
                invalid,
                multiple,
            }
        };
        self.progress.paint(
            Point::new(
                origin.x + self.progress_origin.x,
                origin.y + self.progress_origin.y,
            ),
            chrome.colors.muted_foreground,
            scene,
        );
        for index in 0..self.choice_rects.len() {
            self.paint_choice(scene, origin, index, &chrome);
        }
        if let Some(rect) = self.input_rect {
            let field_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let field_size = Size::new(rect.width(), rect.height());
            if chrome.colors.dark {
                scene.fill_rounded_rect(
                    field_origin,
                    field_size,
                    chrome.radius_lg,
                    style::scale_alpha(chrome.colors.input, DARK_FILL_ALPHA),
                );
            }
        }
        if let Some(error_origin) = self.error_origin {
            self.error.paint(
                Point::new(origin.x + error_origin.x, origin.y + error_origin.y),
                chrome.colors.destructive,
                scene,
            );
        }

        // Phase 2: the pods, over the boxes they sit in.
        for pod in self.pods.iter_mut() {
            pod.paint_child(ctx, scene);
        }

        // Phase 3: what strokes *over* a pod's own opaque fill, plus the actions.
        let field_focused = self.input_focused();
        let theme = Theme::from_paint_ctx(ctx);
        if let Some(rect) = self.input_rect {
            let field_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let field_size = Size::new(rect.width(), rect.height());
            let border = style::focus_border(chrome.colors.input, field_focused, theme);
            stroke_border(scene, field_origin, field_size, chrome.radius_lg, border);
            if field_focused {
                style::draw_focus_ring(
                    scene,
                    field_origin,
                    field_size,
                    chrome.radius_lg,
                    chrome.ring,
                );
            }
        }
        self.paint_actions(scene, origin, &chrome.colors, chrome.radius_md, theme);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in self.pods.iter_mut() {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            return self.handle_key(ctx, event, key);
        }
        // Ime composition and the clipboard verbs an `EditCommand` carries are
        // both focus-routed like `Key` (handled above) and belong to the
        // free-text field outright — branch on the shared predicate rather
        // than enumerating `Ime`/`EditCommand` separately.
        if event.is_focus_routed() {
            return self.route_to_input(ctx, event);
        }
        match event {
            InputEvent::Pointer(p) => self.handle_pointer(ctx, event, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let title = self.item().map(|item| item.title.clone());
        let multiple = self.item().is_some_and(|item| item.multiple);
        let choice_count = self.choices().len();
        let invalid = self.is_invalid();
        ctx.push_container(
            Role::Group,
            |node| {
                if let Some(title) = &title {
                    node.set_label(title.as_str());
                }
            },
            |ctx| {
                ctx.push_node(Role::Label, |node| {
                    node.set_value(self.progress.content());
                });
                let mut index = 0usize;
                if let Some(pod) = self.pods.get(index) {
                    pod.semantics_child(ctx);
                    index += 1;
                }
                if self.has_description
                    && let Some(pod) = self.pods.get(index)
                {
                    pod.semantics_child(ctx);
                    index += 1;
                }
                let group_role = if multiple {
                    Role::Group
                } else {
                    Role::RadioGroup
                };
                let choice_role = if multiple {
                    Role::CheckBox
                } else {
                    Role::RadioButton
                };
                let base = index;
                ctx.push_container(
                    group_role,
                    |_node| {},
                    |ctx| {
                        for choice in 0..choice_count {
                            let pod = self.pods.get(base + choice);
                            let checked = self.choice_checked(choice);
                            let enabled = self.choice_enabled(choice);
                            ctx.push_container(
                                choice_role,
                                |node| {
                                    if let Some(label) =
                                        self.choices().get(choice).map(|c| c.label.as_str())
                                    {
                                        node.set_label(label);
                                    }
                                    if multiple {
                                        node.set_toggled(Toggled::from(checked));
                                    } else {
                                        node.set_selected(checked);
                                    }
                                    if enabled {
                                        node.add_action(Action::Click);
                                    } else {
                                        node.set_disabled();
                                    }
                                },
                                |ctx| {
                                    if let Some(pod) = pod {
                                        pod.semantics_child(ctx);
                                    }
                                },
                            );
                        }
                    },
                );
                index = base + choice_count;
                if self.has_input
                    && let Some(pod) = self.pods.get(index)
                {
                    pod.semantics_child(ctx);
                }
                if invalid {
                    ctx.push_node(Role::Alert, |node| {
                        node.set_label(self.error_text());
                    });
                }
                for button in NavButton::ALL {
                    if self.nav_rects[button.index()].is_none() {
                        continue;
                    }
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(button.label());
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(pods);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, EditCommand, EventOutcome, Modifiers, PointerButton, SemanticsUpdate,
        scene::GlyphRun,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    /// The width every test lays the flow out at.
    const WIDTH: f64 = 320.0;

    /// Records the ops these tests assert on: rounded-rect fills, stroked paths
    /// (as `(bbox, width, color)`), shadows and glyph inks.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(
            &mut self,
            origin: Point,
            size: Size,
            radius: f64,
            std_dev: f64,
            color: Color,
        ) {
            self.shadows.push((origin, size, radius, std_dev, color));
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    /// The app state a flow is controlled from, plus a log of everything the
    /// widget reported.
    #[derive(Default)]
    struct Flow {
        current: usize,
        answers: Vec<QuestionnaireAnswer>,
        answered: Vec<QuestionnaireAnswerEvent>,
        navigated: Vec<usize>,
        submits: u32,
    }

    /// Two required single-select items and one optional multi-select item with a
    /// free-text answer — enough shape to exercise every visibility rule.
    fn items() -> Vec<QuestionnaireItem> {
        vec![
            questionnaire_item("direction", "What should we build next?")
                .description("Choose a direction.")
                .required(true)
                .choices(vec![
                    questionnaire_choice("timeline", "Tool call timeline")
                        .description("Show what ran."),
                    questionnaire_choice("approvals", "Approval checkpoints"),
                    questionnaire_choice("handoffs", "Sub-agent handoffs").disabled(true),
                ]),
            questionnaire_item("signals", "What should an update include?")
                .multiple(true)
                .input("Describe another signal…")
                .choices(vec![
                    questionnaire_choice("progress", "Progress"),
                    questionnaire_choice("risks", "Risks"),
                ]),
            questionnaire_item("timing", "When should work begin?")
                .required(true)
                .choices(vec![
                    questionnaire_choice("now", "Start now"),
                    questionnaire_choice("later", "Next cycle"),
                ]),
        ]
    }

    fn view(current: usize, answers: Vec<QuestionnaireAnswer>) -> QuestionnaireView<Flow> {
        questionnaire::<Flow>(items(), current, answers)
            .shortcuts(QuestionnaireShortcuts::Letters)
            .on_answer(|state: &mut Flow, event| state.answered.push(event))
            .on_navigate(|state: &mut Flow, index| state.navigated.push(index))
            .on_submit(|state: &mut Flow| state.submits += 1)
    }

    fn build(view: &QuestionnaireView<Flow>) -> QuestionnaireWidget {
        let mut counter = 0u64;
        View::<Flow>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(widget: &mut QuestionnaireWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(WIDTH, f64::INFINITY)),
        )
    }

    /// A laid-out widget for `current`/`answers`, ready for event dispatch.
    fn ready(current: usize, answers: Vec<QuestionnaireAnswer>) -> (QuestionnaireWidget, Size) {
        let mut widget = build(&view(current, answers));
        let size = layout(&mut widget);
        (widget, size)
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        widget: &mut QuestionnaireWidget,
        size: Size,
        state: &mut Flow,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        widget.event(&mut ctx, event)
    }

    /// The centre of choice `index`'s box.
    fn choice_center(widget: &QuestionnaireWidget, index: usize) -> Point {
        widget.choice_rects[index].center()
    }

    /// The centre of `button`'s box.
    fn nav_center(widget: &QuestionnaireWidget, button: NavButton) -> Point {
        widget.nav_rects[button.index()]
            .expect("the button is visible")
            .center()
    }

    /// Press and release inside `target`'s resolved box.
    fn click(
        widget: &mut QuestionnaireWidget,
        size: Size,
        state: &mut Flow,
        target: QuestionnaireTarget,
    ) {
        let point = match target {
            QuestionnaireTarget::Choice(index) => choice_center(widget, index),
            QuestionnaireTarget::Nav(button) => nav_center(widget, button),
        };
        dispatch(
            widget,
            size,
            state,
            &pointer(PointerPhase::Down, point.x, point.y),
        );
        dispatch(
            widget,
            size,
            state,
            &pointer(PointerPhase::Up, point.x, point.y),
        );
    }

    fn answered(values: &[&str]) -> QuestionnaireAnswer {
        QuestionnaireAnswer {
            values: values.iter().map(|v| (*v).to_string()).collect(),
            ..QuestionnaireAnswer::default()
        }
    }

    // ---- Data model -------------------------------------------------------

    #[test]
    fn answer_status_ranks_skipped_over_answered_over_unanswered() {
        assert_eq!(
            QuestionnaireAnswer::default().status(),
            QuestionnaireStatus::Unanswered
        );
        assert_eq!(answered(&["a"]).status(), QuestionnaireStatus::Answered);
        assert_eq!(
            QuestionnaireAnswer {
                text: "  ".to_string(),
                ..QuestionnaireAnswer::default()
            }
            .status(),
            QuestionnaireStatus::Unanswered,
            "blank text is not an answer"
        );
        assert_eq!(
            QuestionnaireAnswer {
                text: "yes".to_string(),
                ..QuestionnaireAnswer::default()
            }
            .status(),
            QuestionnaireStatus::Answered
        );
        assert_eq!(
            QuestionnaireAnswer {
                values: vec!["a".to_string()],
                skipped: true,
                ..QuestionnaireAnswer::default()
            }
            .status(),
            QuestionnaireStatus::Skipped
        );
    }

    #[test]
    fn shortcut_badges_skip_disabled_choices_and_match_case_insensitively() {
        let (widget, _) = ready(0, Vec::new());
        // The third choice is disabled, so it never gets a key.
        assert_eq!(
            widget.shortcut_labels,
            vec![Some("A".to_string()), Some("B".to_string()), None]
        );
        assert_eq!(
            QuestionnaireShortcuts::Letters.shortcut_for_key("b"),
            Some("B".to_string())
        );
        assert_eq!(QuestionnaireShortcuts::Letters.shortcut_for_key("1"), None);
        assert_eq!(
            QuestionnaireShortcuts::Numbers.shortcut_for_key("3"),
            Some("3".to_string())
        );
        assert_eq!(QuestionnaireShortcuts::None.shortcut_for_key("a"), None);
    }

    // ---- Layout -----------------------------------------------------------

    #[test]
    fn a_choice_row_clears_the_min_height_and_spans_the_flow() {
        let (widget, size) = ready(0, Vec::new());
        assert_eq!(size.width, WIDTH);
        assert_eq!(widget.choice_rects.len(), 3);
        for rect in &widget.choice_rects {
            assert_eq!(rect.width(), WIDTH);
            assert!(rect.height() >= QUESTIONNAIRE_CHOICE_MIN_HEIGHT);
        }
        // `gap-2` between rows, and the first choice's description makes it the
        // tallest of the three.
        assert_eq!(
            widget.choice_rects[1].y0 - widget.choice_rects[0].y1,
            CHOICES_GAP
        );
        assert!(widget.choice_rects[0].height() > widget.choice_rects[1].height());
        // The content pod is inset past the indicator.
        assert_eq!(
            widget.pods[2].origin().x,
            CHOICE_PAD_X + INDICATOR_SIZE + CHOICE_GAP
        );
    }

    #[test]
    fn an_unbounded_width_falls_back_to_the_flow_default() {
        let mut widget = build(&view(0, Vec::new()));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(f64::INFINITY, f64::INFINITY)),
        );
        assert_eq!(size.width, UNBOUNDED_WIDTH);
    }

    #[test]
    fn the_free_text_field_is_h8_and_the_last_pod() {
        let (widget, _) = ready(1, Vec::new());
        let rect = widget.input_rect.expect("the second item has an input");
        assert_eq!(rect.height(), QUESTIONNAIRE_INPUT_HEIGHT);
        assert_eq!(rect.width(), WIDTH);
        assert_eq!(widget.pods.last().unwrap().size().height, rect.height());
        // No input on the items that author none.
        let (bare, _) = ready(0, Vec::new());
        assert!(bare.input_rect.is_none());
    }

    #[test]
    fn the_action_row_places_previous_leading_and_the_forward_button_trailing() {
        // The middle item: every button but Submit is live.
        let (widget, size) = ready(1, Vec::new());
        let previous = widget.nav_rects[NavButton::Previous.index()].expect("visible");
        let skip = widget.nav_rects[NavButton::Skip.index()].expect("visible");
        let next = widget.nav_rects[NavButton::Next.index()].expect("visible");
        assert!(widget.nav_rects[NavButton::Submit.index()].is_none());
        assert_eq!(previous.x0, 0.0);
        assert_eq!(next.x1, WIDTH);
        assert_eq!(next.x0 - skip.x1, ACTIONS_GAP);
        assert_eq!(previous.y1, size.height);
        assert_eq!(previous.height(), BUTTON_HEIGHT);
    }

    #[test]
    fn visibility_follows_the_sources_navigation_state() {
        // First item: no Previous, and it is required so no Skip.
        let (first, _) = ready(0, Vec::new());
        assert!(first.nav_rects[NavButton::Previous.index()].is_none());
        assert!(first.nav_rects[NavButton::Skip.index()].is_none());
        assert!(first.nav_rects[NavButton::Next.index()].is_some());
        // Last item: Submit replaces Next in the same column.
        let (last, _) = ready(2, Vec::new());
        assert!(last.nav_rects[NavButton::Next.index()].is_none());
        let submit = last.nav_rects[NavButton::Submit.index()].expect("visible");
        assert_eq!(submit.x1, WIDTH);
    }

    #[test]
    fn progress_reflects_the_controlled_index() {
        for index in 0..3 {
            let (widget, _) = ready(index, Vec::new());
            assert_eq!(
                widget.progress.content(),
                format!("Question {} of 3", index + 1)
            );
        }
    }

    // ---- Controlled round-trips -------------------------------------------

    #[test]
    fn a_choice_release_reports_the_value_without_self_mutating() {
        let (mut widget, size) = ready(0, Vec::new());
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Choice(1),
        );

        assert_eq!(state.answered.len(), 1);
        assert_eq!(state.answered[0].item, 0);
        assert_eq!(state.answered[0].answer.values, vec!["approvals"]);
        assert!(widget.answers.is_empty(), "the app owns `answers`");
        assert_eq!(widget.captured, None);
    }

    #[test]
    fn a_single_select_replaces_and_a_multi_select_accumulates() {
        let (mut widget, size) = ready(0, vec![answered(&["timeline"])]);
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Choice(1),
        );
        assert_eq!(state.answered[0].answer.values, vec!["approvals"]);

        let mut answers = vec![QuestionnaireAnswer::default(), answered(&["progress"])];
        let (mut multi, multi_size) = ready(1, answers.clone());
        let mut state = Flow::default();
        click(
            &mut multi,
            multi_size,
            &mut state,
            QuestionnaireTarget::Choice(1),
        );
        assert_eq!(state.answered[0].answer.values, vec!["progress", "risks"]);

        // ...and a second press on a selected checkbox removes it.
        answers[1] = answered(&["progress", "risks"]);
        let (mut multi, multi_size) = ready(1, answers);
        let mut state = Flow::default();
        click(
            &mut multi,
            multi_size,
            &mut state,
            QuestionnaireTarget::Choice(0),
        );
        assert_eq!(state.answered[0].answer.values, vec!["risks"]);
    }

    #[test]
    fn a_disabled_choice_takes_no_press_and_the_gap_between_rows_is_dead() {
        let (mut widget, size) = ready(0, Vec::new());
        let mut state = Flow::default();
        let disabled = choice_center(&widget, 2);
        assert_eq!(
            dispatch(
                &mut widget,
                size,
                &mut state,
                &pointer(PointerPhase::Down, disabled.x, disabled.y)
            ),
            EventResult::Ignored
        );
        assert_eq!(widget.captured, None);

        let gap_y = widget.choice_rects[0].y1 + CHOICES_GAP / 2.0;
        assert_eq!(
            dispatch(
                &mut widget,
                size,
                &mut state,
                &pointer(PointerPhase::Down, 40.0, gap_y)
            ),
            EventResult::Ignored
        );
        assert!(state.answered.is_empty());
    }

    #[test]
    fn a_release_that_drifted_off_the_armed_target_never_fires() {
        let (mut widget, size) = ready(0, Vec::new());
        let mut state = Flow::default();
        let first = choice_center(&widget, 0);
        let second = choice_center(&widget, 1);
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Down, first.x, first.y),
        );
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Up, second.x, second.y),
        );
        assert!(state.answered.is_empty());

        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Down, first.x, first.y),
        );
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Cancel, first.x, first.y),
        );
        assert!(state.answered.is_empty());
        assert_eq!(widget.captured, None);
    }

    #[test]
    fn next_is_blocked_until_the_item_validates_and_latches_the_error() {
        let (mut widget, size) = ready(0, Vec::new());
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Next),
        );
        assert!(state.navigated.is_empty(), "an unanswered item blocks Next");
        assert!(widget.validation_attempted);
        assert!(widget.is_invalid());
        assert_eq!(widget.error_text(), "Choose an answer to continue.");

        // Answered: the same press now reports the next index and the error
        // clears without waiting for another attempt.
        let (mut widget, size) = ready(0, vec![answered(&["timeline"])]);
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Next),
        );
        assert_eq!(state.navigated, vec![1]);
        assert!(!widget.is_invalid());
    }

    #[test]
    fn an_optional_item_still_blocks_next_but_offers_the_skip_wording() {
        let (mut widget, size) = ready(1, vec![QuestionnaireAnswer::default(); 2]);
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Next),
        );
        assert!(state.navigated.is_empty());
        assert_eq!(
            widget.error_text(),
            "Choose an answer or skip this question."
        );
    }

    #[test]
    fn skip_reports_a_skipped_answer_and_then_advances() {
        let (mut widget, size) = ready(1, vec![QuestionnaireAnswer::default(); 2]);
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Skip),
        );
        assert_eq!(state.answered.len(), 1);
        assert!(state.answered[0].answer.skipped);
        assert!(state.answered[0].answer.values.is_empty());
        assert_eq!(state.navigated, vec![2]);
        assert_eq!(state.submits, 0);

        // A skipped optional item validates, so Next then moves on unblocked.
        let skipped = QuestionnaireAnswer {
            skipped: true,
            ..QuestionnaireAnswer::default()
        };
        let (widget, _) = ready(1, vec![QuestionnaireAnswer::default(), skipped]);
        assert!(widget.is_valid());
    }

    #[test]
    fn submit_fires_only_once_the_last_item_validates() {
        let mut answers = vec![
            answered(&["timeline"]),
            answered(&["risks"]),
            QuestionnaireAnswer::default(),
        ];
        let (mut widget, size) = ready(2, answers.clone());
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Submit),
        );
        assert_eq!(state.submits, 0);
        assert!(widget.is_invalid());

        answers[2] = answered(&["now"]);
        let (mut widget, size) = ready(2, answers);
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Submit),
        );
        assert_eq!(state.submits, 1);
        assert!(state.navigated.is_empty());
    }

    #[test]
    fn previous_reports_the_earlier_index_without_validating() {
        let (mut widget, size) = ready(2, vec![QuestionnaireAnswer::default(); 3]);
        let mut state = Flow::default();
        click(
            &mut widget,
            size,
            &mut state,
            QuestionnaireTarget::Nav(NavButton::Previous),
        );
        assert_eq!(state.navigated, vec![1]);
        assert!(!widget.validation_attempted, "going back never validates");
    }

    // ---- Keyboard ---------------------------------------------------------

    #[test]
    fn a_shortcut_key_selects_its_choice_and_an_unassigned_one_is_ignored() {
        let (mut widget, size) = ready(0, Vec::new());
        let mut state = Flow::default();
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Character("b".into())),
        );
        assert_eq!(state.answered[0].answer.values, vec!["approvals"]);
        assert_eq!(widget.focused_choice, 1);

        // `C` belongs to the disabled third choice, so it was never handed out.
        assert_eq!(
            dispatch(
                &mut widget,
                size,
                &mut state,
                &key(Key::Character("c".into()))
            ),
            EventResult::Ignored
        );
        assert_eq!(state.answered.len(), 1);
    }

    #[test]
    fn arrow_keys_rove_the_choices_and_traverse_the_items() {
        let (mut widget, size) = ready(0, vec![answered(&["timeline"])]);
        let mut state = Flow::default();
        assert_eq!(widget.focused_choice, 0);

        // Down moves and — this being a radio item — selects as it moves.
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(widget.focused_choice, 1);
        assert_eq!(state.answered[0].answer.values, vec!["approvals"]);
        // The third choice is disabled, so the next step wraps past it.
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::ArrowDown)),
        );
        assert_eq!(widget.focused_choice, 0);
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::ArrowUp)),
        );
        assert_eq!(widget.focused_choice, 1);

        // Right traverses forward on an answered item; Enter confirms the same.
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(state.navigated, vec![1]);
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::Enter)),
        );
        assert_eq!(state.navigated, vec![1, 1]);
        assert_eq!(
            widget.answers,
            vec![answered(&["timeline"])],
            "no key ever wrote an answer down"
        );
        assert_eq!(widget.current, 0, "and none moved the item either");
    }

    #[test]
    fn the_forward_arrow_is_inert_on_an_untouched_item_and_left_goes_back() {
        let (mut widget, size) = ready(1, vec![QuestionnaireAnswer::default(); 2]);
        let mut state = Flow::default();
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert!(state.navigated.is_empty());
        assert!(
            !widget.validation_attempted,
            "an untouched item is stepped past through Next, not the arrow"
        );

        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(state.navigated, vec![0]);
    }

    #[test]
    fn space_reports_the_roving_choice_and_an_unrelated_key_is_ignored() {
        let (mut widget, size) = ready(0, Vec::new());
        let mut state = Flow::default();
        widget.focused_choice = 1;
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Character(" ".into())),
        );
        assert_eq!(state.answered[0].answer.values, vec!["approvals"]);
        assert_eq!(
            dispatch(
                &mut widget,
                size,
                &mut state,
                &key(Key::Named(NamedKey::Tab))
            ),
            EventResult::Ignored
        );
        assert_eq!(state.answered.len(), 1);
    }

    /// `EditCommand::Paste` is focus-routed exactly like `Key`/`Ime`: a
    /// clipboard paste dispatched at the widget while the free-text field
    /// holds focus must reach it, closing the gap where a `Ctrl+V` chord's
    /// `ctx.request_paste()` succeeds but the shell's separate top-level
    /// `EditCommand::Paste(text)` dispatch it triggers is then swallowed here.
    #[test]
    fn a_paste_edit_command_reaches_the_focused_free_text_field() {
        let (mut widget, size) = ready(1, Vec::new());
        let mut state = Flow::default();
        let rect = widget.input_rect.expect("the second item has an input");
        // Tap the free-text field so it holds the recorded focus path.
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Down, rect.center().x, rect.center().y),
        );
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Up, rect.center().x, rect.center().y),
        );
        assert!(
            widget.input_focused(),
            "the tap focused the free-text field"
        );
        dispatch(
            &mut widget,
            size,
            &mut state,
            &InputEvent::EditCommand(EditCommand::Paste("hello".to_string())),
        );
        assert_eq!(
            state.answered.last().map(|e| e.answer.text.as_str()),
            Some("hello"),
            "the pasted text must reach the focused free-text field"
        );
    }

    /// Guard against over-forwarding: Enter confirms from anywhere — including
    /// from inside the free-text field (module docs) — so `handle_key` must
    /// keep intercepting it before `is_focus_routed()`'s wider catch-all ever
    /// gets a look, even while the field holds focus.
    #[test]
    fn enter_confirms_and_is_not_forwarded_while_the_field_is_focused() {
        // The item must already validate (it is not required, but an
        // unanswered item still fails `is_valid`), so Enter reaches
        // `confirm`'s navigate branch rather than its validation-latch one —
        // either way proves Enter never reaches the field, but this keeps the
        // assertion about *where the event went*, not about validity.
        let (mut widget, size) = ready(
            1,
            vec![QuestionnaireAnswer::default(), answered(&["progress"])],
        );
        let mut state = Flow::default();
        let rect = widget.input_rect.expect("the second item has an input");
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Down, rect.center().x, rect.center().y),
        );
        dispatch(
            &mut widget,
            size,
            &mut state,
            &pointer(PointerPhase::Up, rect.center().x, rect.center().y),
        );
        assert!(
            widget.input_focused(),
            "the tap focused the free-text field"
        );
        dispatch(
            &mut widget,
            size,
            &mut state,
            &key(Key::Named(NamedKey::Enter)),
        );
        assert_eq!(
            state.navigated,
            vec![2],
            "Enter confirmed the item and advanced, rather than reaching the field"
        );
        assert!(
            state.answered.is_empty(),
            "Enter must not be routed to the free-text field as text"
        );
    }

    // ---- Rebuild ----------------------------------------------------------

    #[test]
    fn moving_to_another_item_re_pods_it_and_drops_the_latched_validation() {
        let mut counter = 0u64;
        let prev = view(0, Vec::new());
        let mut widget = View::<Flow>::build(&prev, &mut BuildCtx::new(&mut counter));
        widget.validation_attempted = true;
        widget.focused_choice = 2;

        let next = view(1, Vec::new());
        let flags =
            View::<Flow>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert_eq!(widget.current, 1);
        assert!(!widget.validation_attempted);
        assert_eq!(widget.focused_choice, 0);
        assert!(
            widget.has_input,
            "the second item authors a free-text answer"
        );
        assert!(!widget.has_description);
        assert!(flags.needs_layout() && flags.needs_paint());
        // title + 2 choices + input, with no description pod.
        assert_eq!(widget.pods.len(), 4);
    }

    #[test]
    fn a_new_answer_list_is_adopted_as_a_repaint() {
        let mut counter = 0u64;
        let prev = view(0, Vec::new());
        let mut widget = View::<Flow>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view(0, vec![answered(&["timeline"])]);
        let flags =
            View::<Flow>::rebuild(&next, &prev, &mut widget, &mut BuildCtx::new(&mut counter));
        assert!(widget.choice_checked(0));
        assert!(flags.needs_paint());
    }

    // ---- Paint / root-driven ----------------------------------------------

    /// One flow under a real `RenderRoot` — focus, hover and the cursor are all
    /// recorded by the root's event pass, so nothing else can exercise them.
    struct Harness {
        root: RenderRoot<Flow, QuestionnaireView<Flow>>,
        state: Flow,
        tcx: TextContext,
    }

    impl Harness {
        fn new(current: usize, answers: Vec<QuestionnaireAnswer>) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: Flow {
                    current,
                    answers,
                    ..Flow::default()
                },
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            h.pass();
            h
        }

        fn pass(&mut self) {
            let mut logic = |state: &mut Flow| view(state.current, state.answers.clone());
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(Size::new(WIDTH, 800.0), &mut self.tcx as &mut dyn Any);
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    /// The widget-local geometry a harness test needs, read off a twin of the
    /// widget the root retains. The twin is laid out the way the root lays out
    /// its own, under the harness's theme and through its text context, because
    /// the theme picks the text's family and so the heights the rows stack from.
    fn probe(h: &mut Harness) -> (Vec<Rect>, [Option<Rect>; 4]) {
        let mut widget = build(&view(h.state.current, h.state.answers.clone()));
        let theme = crate::theme();
        let mut lctx =
            LayoutCtx::with_text_context(&mut h.tcx as &mut dyn Any).with_theme(&theme as &dyn Any);
        widget.layout(
            &mut lctx,
            &BoxConstraints::loose(Size::new(WIDTH, f64::INFINITY)),
        );
        (widget.choice_rects.clone(), widget.nav_rects)
    }

    #[test]
    fn a_checked_choice_takes_the_muted_fill_and_the_primary_indicator() {
        let scheme = crate::theme();
        let scheme = scheme.scheme();
        let mut h = Harness::new(0, vec![answered(&["timeline"])]);
        let rec = h.paint();

        // The checked row's `bg-muted` box…
        assert!(
            rec.rrects
                .iter()
                .any(|(_, size, _, color)| size.width == WIDTH
                    && *color == scheme.surface_container_highest),
            "`data-checked:bg-muted`"
        );
        // …and its `bg-primary` indicator, with the dot on top.
        assert!(rec.rrects.iter().any(|(_, size, _, color)| *size
            == Size::new(INDICATOR_SIZE, INDICATOR_SIZE)
            && *color == scheme.primary));
        assert!(
            rec.rrects.iter().any(|(_, size, _, color)| *size
                == Size::new(INDICATOR_DOT_SIZE, INDICATOR_DOT_SIZE)
                && *color == scheme.on_primary),
            "a radio item paints a dot, never a check"
        );
        // The checked border is `border-primary/40`.
        let expected = style::scale_alpha(scheme.primary, CHOICE_CHECKED_BORDER_ALPHA);
        assert!(
            rec.strokes
                .iter()
                .any(|(_, width, color)| *width == style::BORDER_WIDTH && *color == expected)
        );
    }

    #[test]
    fn a_multi_select_item_paints_a_rounded_indicator_and_a_check() {
        let mut h = Harness::new(
            1,
            vec![QuestionnaireAnswer::default(), answered(&["progress"])],
        );
        let rec = h.paint();
        assert!(
            rec.rrects.iter().any(|(_, size, radius, _)| *size
                == Size::new(INDICATOR_SIZE, INDICATOR_SIZE)
                && *radius == INDICATOR_RADIUS),
            "`rounded-[4px]`, not the radio's circle"
        );
        // The check glyph is the one stroke carrying lucide's scaled width.
        let check_stroke = CHECK_STROKE_VIEWBOX * INDICATOR_CHECK_SIZE / ICON_VIEWBOX;
        assert!(
            rec.strokes
                .iter()
                .any(|(_, width, _)| (*width - check_stroke).abs() < 1e-9),
            "a radio's dot would be a fill, not a stroke"
        );
    }

    #[test]
    fn hovering_a_choice_swaps_its_fill_and_resolves_the_pointer_cursor() {
        let mut h = Harness::new(0, Vec::new());
        let (rects, _) = probe(&mut h);
        let point = rects[1].center();

        assert_eq!(h.root.cursor(), CursorIcon::Default);
        h.dispatch(&pointer(PointerPhase::Move, point.x, point.y));
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);
        let hover = style::with_alpha(
            crate::theme().scheme().surface_container_highest,
            CHOICE_HOVER_ALPHA,
        );
        assert!(
            h.paint()
                .rrects
                .iter()
                .any(|(_, _, _, color)| *color == hover),
            "`hover:bg-muted/50`"
        );

        // Off every target: nobody asks, and the fill goes back.
        let gap_y = rects[0].y1 + CHOICES_GAP / 2.0;
        h.dispatch(&pointer(PointerPhase::Move, 40.0, gap_y));
        assert_eq!(h.root.cursor(), CursorIcon::Default);
        assert!(
            !h.paint()
                .rrects
                .iter()
                .any(|(_, _, _, color)| *color == hover)
        );
    }

    #[test]
    fn the_focus_ring_lands_on_the_pressed_choice_only() {
        let mut h = Harness::new(0, Vec::new());
        let (rects, _) = probe(&mut h);
        assert!(
            !h.paint()
                .strokes
                .iter()
                .any(|(_, width, _)| *width == style::FOCUS_RING_WIDTH)
        );

        let point = rects[1].center();
        h.dispatch(&pointer(PointerPhase::Down, point.x, point.y));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        let (bbox, _, color) = rec
            .strokes
            .iter()
            .find(|(_, width, _)| *width == style::FOCUS_RING_WIDTH)
            .copied()
            .expect("a focus ring");
        assert_eq!(color.components[3], style::FOCUS_RING_OPACITY);
        assert!(bbox.y0 < rects[1].y0 && bbox.y1 > rects[1].y1);
    }

    #[test]
    fn the_outline_actions_carry_a_shadow_and_the_forward_one_does_not() {
        let mut h = Harness::new(1, vec![QuestionnaireAnswer::default(); 2]);
        let scheme = crate::theme();
        let scheme = scheme.scheme();
        let rec = h.paint();
        // Previous + Skip are `variant="outline"`, so exactly two `shadow-xs`.
        assert_eq!(rec.shadows.len(), 2);
        assert!(
            rec.shadows
                .iter()
                .all(|(_, _, _, std_dev, _)| *std_dev == style::SHADOW_XS.std_dev)
        );
        // Next is solid `bg-primary`.
        assert!(
            rec.rrects
                .iter()
                .any(|(_, size, _, color)| size.height == BUTTON_HEIGHT
                    && *color == scheme.primary)
        );
    }

    #[test]
    fn the_error_line_appears_only_after_a_blocked_press_and_asks_for_a_relayout() {
        let mut h = Harness::new(0, Vec::new());
        let (_, navs) = probe(&mut h);
        let point = navs[NavButton::Next.index()]
            .expect("Next is visible")
            .center();
        let before = h.paint().inks.len();

        h.dispatch(&pointer(PointerPhase::Down, point.x, point.y));
        h.dispatch(&pointer(PointerPhase::Up, point.x, point.y));
        // The first paint after the block reserves no room yet, so it asks for
        // layout; the next pass has the line measured and painted.
        h.paint();
        h.pass();
        let rec = h.paint();
        assert!(rec.inks.len() > before);
        assert!(
            rec.inks.contains(&crate::theme().scheme().error),
            "`text-destructive`"
        );
        assert_eq!(h.state.navigated, Vec::<usize>::new());
    }

    #[test]
    fn semantics_yields_a_labelled_group_of_radios_over_the_visible_actions() {
        let h = Harness::new(0, vec![answered(&["timeline"])]);
        let update = h.semantics();
        let group = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::RadioGroup)
            .expect("a Role::RadioGroup node");
        assert_eq!(group.1.children().len(), 3);

        let radios: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::RadioButton)
            .collect();
        assert_eq!(radios.len(), 3);
        assert_eq!(radios[0].1.label(), Some("Tool call timeline"));
        assert_eq!(radios[0].1.is_selected(), Some(true));
        assert_eq!(radios[1].1.is_selected(), Some(false));
        assert!(radios[1].1.supports_action(Action::Click));
        assert!(radios[2].1.is_disabled(), "the third choice is disabled");

        // Only the buttons the row actually renders are published.
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .filter_map(|(_, n)| n.label())
            .collect();
        assert_eq!(
            buttons,
            vec!["Next"],
            "no Previous, and the item is required"
        );

        // The progress line is published as its own labelled node.
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Label && n.value() == Some("Question 1 of 3"))
        );
    }

    #[test]
    fn the_free_text_field_publishes_its_own_node_and_keeps_the_text_cursor() {
        let mut h = Harness::new(1, vec![QuestionnaireAnswer::default(); 2]);
        let update = h.semantics();
        assert!(
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::TextInput),
            "the wrapped baseline field contributes its own node"
        );

        let mut widget = build(&view(1, vec![QuestionnaireAnswer::default(); 2]));
        layout(&mut widget);
        let rect = widget.input_rect.expect("an input");
        h.dispatch(&pointer(
            PointerPhase::Move,
            rect.center().x,
            rect.center().y,
        ));
        assert_eq!(h.root.cursor(), CursorIcon::Text);
    }

    // ---- Typeface: the flow's text follows the live theme -----------------

    /// A flow holding only text that takes the theme's family: the progress
    /// line, the current item's title and description, choice labels with and
    /// without a description, and the Previous/Skip/Submit labels (the second
    /// item is current and optional). It leaves out the shortcut badges, which
    /// are monospace by design, and the free-text field, which is the baseline
    /// `text_input` (see `QuestionnaireView::input_view`).
    #[cfg(feature = "bundled-fonts")]
    fn themed_flow(_: &mut ()) -> QuestionnaireView<()> {
        questionnaire(
            vec![
                questionnaire_item("direction", "What should we build next?")
                    .choices(vec![questionnaire_choice("timeline", "Tool call timeline")]),
                questionnaire_item("timing", "When should work begin?")
                    .description("Pick the closest option.")
                    .choices(vec![
                        questionnaire_choice("now", "Start now").description("This cycle."),
                        questionnaire_choice("later", "Next cycle"),
                    ]),
            ],
            1,
            Vec::new(),
        )
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_flow_text_paints_in_the_theme_face() {
        crate::text::typeface_probe::assert_paints_in_the_theme_face(
            "a questionnaire's text",
            themed_flow,
        );
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_flow_text_follows_a_live_theme_swap() {
        crate::text::typeface_probe::assert_follows_a_live_theme_swap(
            "a questionnaire's text",
            themed_flow,
        );
    }

    /// The error line shows only after a blocked Next, and the shared probe
    /// dispatches no input. So the validation is latched directly, and the
    /// widget is laid out and painted under a theme with both bundled faces
    /// registered. The line's runs (the only ones in the destructive ink) must
    /// shape in the family the theme's `BodyMedium` role names, and follow that
    /// role when a second theme on the same text context changes it.
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_error_line_takes_the_family_of_the_themes_body_role() {
        use crate::tokens::fonts::{INTER_VARIABLE_INDEX, JETBRAINS_MONO_VARIABLE_INDEX};
        use crate::tokens::{font_data, mono_family};

        /// The face of every glyph run painted in `ink`.
        struct InkFaces {
            ink: Color,
            faces: Vec<&'static str>,
        }

        impl PaintScene for InkFaces {
            fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
            fn draw_text(&mut self, _origin: Point, _text: &str) {}
            fn draw_glyph_run(&mut self, run: GlyphRun) {
                if !matches!(run.brush, Brush::Solid(color) if color == self.ink) {
                    return;
                }
                let bytes = run.font.font().data.as_ref();
                self.faces
                    .push(if bytes == font_data()[INTER_VARIABLE_INDEX] {
                        "Inter"
                    } else if bytes == font_data()[JETBRAINS_MONO_VARIABLE_INDEX] {
                        "JetBrains Mono"
                    } else {
                        "another font"
                    });
            }
        }

        fn error_faces(
            widget: &mut QuestionnaireWidget,
            tcx: &mut TextContext,
            theme: &Theme,
        ) -> Vec<&'static str> {
            let size = {
                let mut lctx =
                    LayoutCtx::with_text_context(tcx as &mut dyn Any).with_theme(theme as &dyn Any);
                widget.layout(
                    &mut lctx,
                    &BoxConstraints::loose(Size::new(WIDTH, f64::INFINITY)),
                )
            };
            let mut rec = InkFaces {
                ink: theme.scheme().error,
                faces: Vec::new(),
            };
            let mut ctx = PaintCtx::new(Point::ORIGIN, size).with_theme(theme as &dyn Any);
            widget.paint(&mut ctx, &mut rec);
            rec.faces
        }

        let mut tcx = TextContext::new();
        for face in font_data() {
            tcx.register_fonts(face.to_vec())
                .expect("a bundled shadcn face registers");
        }
        let mut widget = build(&view(0, Vec::new()));
        widget.validation_attempted = true;

        let before = error_faces(&mut widget, &mut tcx, &crate::theme());
        assert!(!before.is_empty(), "the latched error line paints");
        assert!(
            before.iter().all(|face| *face == "Inter"),
            "under shadcn's theme the error line shaped in {before:?}"
        );

        let mut swapped = crate::theme();
        swapped.type_scale.body_medium.family = mono_family();
        let after = error_faces(&mut widget, &mut tcx, &swapped);
        assert!(!after.is_empty(), "the latched error line still paints");
        assert!(
            after.iter().all(|face| *face == "JetBrains Mono"),
            "with `BodyMedium` naming JetBrains Mono the error line shaped in {after:?}"
        );
    }
}
