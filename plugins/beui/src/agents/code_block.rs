//! Ports beUI's `code-block` agent-interface part.
//!
//! **Source:** `components/agents/code-block.tsx` (and the shared
//! `components/agents/agent-code.tsx` it renders each line through) of the beUI
//! monorepo, rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | wrapper `rounded-2xl bg-muted/80 text-sm` | [`CODE_BLOCK_RADIUS`] over the muted surface |
//! | header `flex h-10 items-center gap-2.5 px-3` | [`CODE_BLOCK_HEADER_HEIGHT`], [`CODE_BLOCK_HEADER_GAP`], [`CODE_BLOCK_HEADER_PADDING_X`] |
//! | filename `font-mono text-xs text-foreground/80` | [`CODE_TEXT_SIZE`] in the mono family, [`CODE_FILENAME_ALPHA`] |
//! | language `text-[10px] uppercase text-muted-foreground/55` | [`CODE_LANGUAGE_SIZE`], [`CODE_LANGUAGE_ALPHA`], upper-cased at build |
//! | `streaming ? "Writing" : "Ready"` + spinner/check | [`CodeBlockStatus`], the arc/check pair |
//! | copy button `size-7 rounded-full`, `whileTap {scale: 0.9}` | [`CODE_COPY_BOX`], [`CODE_COPY_PRESS_SCALE`] on `SPRING_PRESS` |
//! | `setCopied(true)` for 1600ms | [`CODE_COPY_FEEDBACK`], latched off the frame clock |
//! | viewport `overflow-auto py-2` `style={{maxHeight}}` | [`CODE_PADDING_Y`], [`CodeBlockView::max_height`] |
//! | line grid `2.75rem minmax(0,1fr)`, `min-h-5` | [`CODE_GUTTER_WIDTH`], [`CODE_LINE_HEIGHT`] |
//! | `highlighted.has(n) && "bg-blue-500/[0.07]"` | [`CodeBlockView::highlight_lines`] at [`CODE_HIGHLIGHT_ALPHA`] |
//! | `AgentCodeLine` token spans | [`CodeSpan`] / [`CodeTokenClass`] |
//!
//! # Syntax highlighting is a seam, not a port
//!
//! Upstream highlights with **shiki** — a full TextMate-grammar tokenizer with
//! two bundled VS Code themes, loaded asynchronously and cached per
//! `(language, source)` pair. There is no equivalent in this workspace and
//! vendoring one is a dependency decision far larger than a component port, so
//! this port ships the *seam* instead of the engine:
//!
//! - the default is **monochrome** — every line is one [`CodeTokenClass::Plain`]
//!   span, which is exactly what upstream renders before its highlighter
//!   resolves (`useAgentCodeTokens` returns `null` until then);
//! - a caller that already has tokens — from its own highlighter, an LSP
//!   semantic-tokens response, or a server that pre-colored the payload — hands
//!   them in through [`CodeBlockView::colored`] as one [`CodeSpan`] list per
//!   line, and each span's [`CodeTokenClass`] resolves through [`CodePalette`]
//!   to a catalog token rather than to a theme-baked hex the way shiki's do.
//!
//! The class list is deliberately small and *semantic* (keyword, string,
//! number, comment, punctuation, function, type) rather than a TextMate scope
//! name: it is what a palette can be defined over, and it keeps the colors
//! inside the catalog's token tables instead of importing a second theme.
//!
//! This is a real degradation against the web original and is recorded as one.
//!
//! # The collapse morph, and why `paint` asks for layout
//!
//! The viewport height is a lane between a collapsed band
//! ([`CODE_BLOCK_COLLAPSED_LINES`] lines) and the expanded one (the natural
//! height, capped by [`CodeBlockView::max_height`]). The lane advances on the
//! clock only `paint` has and the height is computed from it in `layout`, so
//! `paint` calls [`PaintCtx::request_layout`] while the lane is in flight and
//! once more on the frame its value actually changed — which covers the landing
//! frame and the `reduce_motion` snap, neither of which reports "still moving".
//! The same construction the catalog's accordion uses.
//!
//! # Degradations against the web original
//!
//! - **No shiki**, above.
//! - **No scrolling, on either axis.** Upstream's viewport is `overflow-auto`;
//!   here a line longer than the box is clipped at the right edge and the
//!   vertical cap is a clip too. There is no scroll container a leaf widget in
//!   this catalog can become.
//! - **No follow-the-tail during streaming.** Upstream scrolls the viewport to
//!   the bottom on every streamed update. With no scroll offset there is
//!   nothing to drive; [`CodeBlockView::follow`] keeps the *effect* by pinning
//!   the visible window to the **last** lines instead of the first.
//! - **`on_copy` is the whole contract.** Upstream falls back to
//!   `navigator.clipboard.writeText` when no handler is given; a plugin-tier
//!   widget has no clipboard (that is the app's clipboard plugin), so the
//!   component reports the press and paints the confirmation, and the app does
//!   the writing.
//! - **No wrap mode.** Upstream's `wrap` prop switches a line to
//!   `whitespace-pre-wrap`; every line here is one shaped run on one row.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color,
    ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size,
    View, Widget, erase_callback, erase_callback_arg,
    text::{FontWeight, TextStyle},
};
use frust::{ColorScheme, FrameTime, Theme};

use crate::motion::Ramp;
use crate::press::{
    Lane, SpringScalar, draw_focus_ring, inside, is_activation_key, press_scale, presses,
};
use crate::style::{self, with_alpha};
use crate::text::{LabelRun, SHAPING_INK};
use crate::tokens::motion::{SPRING_PANEL, SPRING_PRESS};
use crate::tokens::{BEUI_LIGHT, BeuiTokens};

/// The header strip's height, in logical px (`h-10`).
pub const CODE_BLOCK_HEADER_HEIGHT: f64 = style::HEIGHT_MD;

/// The gap between the header's parts, in logical px (`gap-2.5`).
pub const CODE_BLOCK_HEADER_GAP: f64 = 10.0;

/// The header's horizontal padding, in logical px (`px-3`).
pub const CODE_BLOCK_HEADER_PADDING_X: f64 = 12.0;

/// The code surface's corner radius (`rounded-2xl`).
pub const CODE_BLOCK_RADIUS: f64 = style::RADIUS_2XL;

/// One code row's height, in logical px (`leading-5` / `min-h-5`).
pub const CODE_LINE_HEIGHT: f64 = 20.0;

/// The code type size, in logical px (`text-xs`).
pub const CODE_TEXT_SIZE: f64 = style::TEXT_XS;

/// The language tag's type size, in logical px (`text-[10px]`).
pub const CODE_LANGUAGE_SIZE: f64 = 10.0;

/// The line-number gutter's width, in logical px (`2.75rem`).
pub const CODE_GUTTER_WIDTH: f64 = 44.0;

/// The gap between the gutter and the code, in logical px (`pl-1`).
pub const CODE_GUTTER_GAP: f64 = 4.0;

/// The gutter's own right padding, in logical px (`pr-3`).
pub const CODE_GUTTER_PADDING: f64 = 12.0;

/// The code viewport's vertical padding, in logical px (`py-2`).
pub const CODE_PADDING_Y: f64 = 8.0;

/// The code's horizontal padding when there is no gutter, in logical px
/// (`pl-4` / `pr-4`).
pub const CODE_PADDING_X: f64 = 16.0;

/// The default height cap on the code viewport, in logical px (`maxHeight =
/// 280`).
pub const CODE_BLOCK_MAX_HEIGHT: f64 = 280.0;

/// How many lines a collapsed block still shows.
///
/// An addition: upstream's block has no collapsed state at all — its siblings
/// `file-diff` and `tool-result` do, and an agent transcript wants the same
/// affordance on a long generated file. Three lines identify the payload
/// without letting it dominate the transcript.
pub const CODE_BLOCK_COLLAPSED_LINES: usize = 3;

/// The copy affordance's box, in logical px (`size-7`).
pub const CODE_COPY_BOX: f64 = 28.0;

/// The scale the copy affordance shrinks to while pressed
/// (`whileTap={{ scale: 0.9 }}`).
pub const CODE_COPY_PRESS_SCALE: f64 = 0.9;

/// How long the copy confirmation stays up (`setTimeout(…, 1600)`).
pub const CODE_COPY_FEEDBACK: Duration = Duration::from_millis(1600);

/// How long the streaming spinner takes to turn once — Tailwind's
/// `animate-spin` (`animation: spin 1s linear infinite`).
pub const CODE_SPINNER_PERIOD: Duration = Duration::from_millis(1000);

/// Alpha of the wash a highlighted line is tinted with
/// (`bg-blue-500/[0.07]`).
pub const CODE_HIGHLIGHT_ALPHA: f32 = 0.07;

/// Alpha the code ink is painted at (`text-foreground/85`).
pub const CODE_PLAIN_ALPHA: f32 = 0.85;

/// Alpha the line numbers are painted at (`text-muted-foreground/35`).
pub const CODE_GUTTER_ALPHA: f32 = 0.35;

/// Alpha the language tag is painted at (`text-muted-foreground/55`).
pub const CODE_LANGUAGE_ALPHA: f32 = 0.55;

/// Alpha the filename is painted at (`text-foreground/80`).
pub const CODE_FILENAME_ALPHA: f32 = 0.8;

/// Alpha the code surface is filled at (`bg-muted/80`).
pub const CODE_SURFACE_ALPHA: f32 = 0.8;

/// Alpha of the hairline under the header (`border-foreground/[0.06]`).
pub const CODE_HAIRLINE_ALPHA: f32 = 0.06;

/// The ramp the collapse/expand height morph plays, both ways.
pub const CODE_BLOCK_MORPH: Ramp = Ramp::spring(SPRING_PANEL);

/// Whether the payload is still arriving (`status`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodeBlockStatus {
    /// `"streaming"` — the agent is still writing. Reads "Writing", turns the
    /// spinner.
    Streaming,
    /// `"complete"` — the payload is final. Reads "Ready", shows the check.
    #[default]
    Complete,
}

impl CodeBlockStatus {
    /// The word the header shows for this status.
    pub const fn label(self) -> &'static str {
        match self {
            CodeBlockStatus::Streaming => "Writing",
            CodeBlockStatus::Complete => "Ready",
        }
    }

    /// Whether the payload is still arriving.
    pub const fn is_streaming(self) -> bool {
        matches!(self, CodeBlockStatus::Streaming)
    }
}

/// What one [`CodeSpan`] *is*, semantically — the colorizer seam described in
/// the [module docs](self).
///
/// Deliberately a short semantic list rather than a TextMate scope name: it is
/// the vocabulary a [`CodePalette`] can be defined over, and it keeps a code
/// panel's colors inside the catalog's own token tables.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CodeTokenClass {
    /// Unclassified text — the monochrome default.
    #[default]
    Plain,
    /// A language keyword (`fn`, `return`, `const`).
    Keyword,
    /// A string or character literal.
    Str,
    /// A numeric or boolean literal.
    Number,
    /// A comment.
    Comment,
    /// Brackets, operators and separators.
    Punctuation,
    /// A called or declared function name.
    Function,
    /// A type name.
    Type,
}

/// One run of code text and what it is.
///
/// The unit [`CodeBlockView::colored`] takes: a caller with its own tokenizer
/// hands one list of these per line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeSpan {
    /// The literal text of the run.
    pub text: String,
    /// What the run is.
    pub class: CodeTokenClass,
}

/// Build one classified [`CodeSpan`].
pub fn code_span(text: impl Into<String>, class: CodeTokenClass) -> CodeSpan {
    CodeSpan {
        text: text.into(),
        class,
    }
}

/// Split `code` into one monochrome [`CodeSpan`] list per line — what a block
/// with no caller-supplied tokens renders, and what upstream renders before its
/// highlighter resolves.
pub fn plain_spans(code: &str) -> Vec<Vec<CodeSpan>> {
    code.split('\n')
        .map(|line| vec![code_span(line, CodeTokenClass::Plain)])
        .collect()
}

/// The resolved ink for each [`CodeTokenClass`], plus the surface a code panel
/// fills.
///
/// Shared with this crate's sibling code-bearing panels — the
/// [diff view](super::file_diff), the [tool result](super::tool_result) and the
/// [tool-approval](super::tool_approval) parameter block — all of which paint
/// the same span vocabulary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodePalette {
    /// The surface a code panel fills (`bg-muted/80`).
    pub surface: Color,
    /// Unclassified code ink.
    pub plain: Color,
    /// Keyword ink.
    pub keyword: Color,
    /// String-literal ink.
    pub string: Color,
    /// Numeric-literal ink.
    pub number: Color,
    /// Comment ink — also the dimmed ink a panel's own chrome uses.
    pub comment: Color,
    /// Punctuation ink.
    pub punctuation: Color,
    /// Function-name ink — also the catalog's accent, which the streaming
    /// status and the highlight wash both read.
    pub function: Color,
    /// Type-name ink.
    pub type_name: Color,
}

impl CodePalette {
    /// The ink `class` paints in.
    pub const fn ink(&self, class: CodeTokenClass) -> Color {
        match class {
            CodeTokenClass::Plain => self.plain,
            CodeTokenClass::Keyword => self.keyword,
            CodeTokenClass::Str => self.string,
            CodeTokenClass::Number => self.number,
            CodeTokenClass::Comment => self.comment,
            CodeTokenClass::Punctuation => self.punctuation,
            CodeTokenClass::Function => self.function,
            CodeTokenClass::Type => self.type_name,
        }
    }
}

/// Resolve the code palette, falling back to the vendored **light** table
/// unthemed — the per-value fallback rule the crate charter sets.
///
/// The brand hues (`--neon`, `--violet`, `--success`, `--warning`) come off
/// [`BeuiTokens`] rather than a `ColorScheme` role, because beUI authors them
/// with no role to fold into; that is the same ladder the badge and toast
/// surfaces climb.
pub fn code_palette(theme: Option<&Theme>) -> CodePalette {
    let tokens = BeuiTokens::resolve(theme);
    let scheme = theme.map(Theme::scheme);
    let role = |pick: fn(&ColorScheme) -> Color, fallback: Color| scheme.map_or(fallback, pick);
    let foreground = role(|s| s.on_surface, BEUI_LIGHT.foreground);
    let muted = role(|s| s.on_surface_variant, BEUI_LIGHT.muted_foreground);
    CodePalette {
        surface: with_alpha(
            role(|s| s.surface_container, BEUI_LIGHT.muted),
            CODE_SURFACE_ALPHA,
        ),
        plain: with_alpha(foreground, CODE_PLAIN_ALPHA),
        keyword: tokens.violet,
        string: tokens.success,
        number: tokens.warning,
        comment: muted,
        punctuation: muted,
        function: role(|s| s.primary, BEUI_LIGHT.primary),
        type_name: tokens.neon,
    }
}

/// The style every code glyph in this crate is shaped with: beUI's mono stack
/// (`--font-mono`, Geist Mono) at `size`, in the shared shaping ink so the
/// shape cache never keys on the color a span is re-brushed to.
pub fn code_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::mono_family(),
        ..TextStyle::new(size as f32, SHAPING_INK)
    }
}

/// The style a panel's small chrome labels are shaped with — the sans family at
/// `size`, medium weight (`text-[10px] font-medium`).
pub fn chrome_style(size: f64) -> TextStyle {
    TextStyle {
        family: crate::tokens::sans_family(),
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(size as f32, SHAPING_INK)
    }
}

/// A view-held callback, erased on build.
type OnState<State> = Rc<dyn Fn(&mut State)>;
/// A view-held one-argument callback, erased on build.
type OnArg<State, A> = Rc<dyn Fn(&mut State, A)>;

/// A declarative beUI code block. See the [module docs](self).
pub struct CodeBlockView<State: 'static> {
    code: String,
    colored: Option<Vec<Vec<CodeSpan>>>,
    language: String,
    filename: Option<String>,
    status: CodeBlockStatus,
    line_numbers: bool,
    highlight_lines: Vec<usize>,
    max_height: f64,
    collapsed: bool,
    follow: bool,
    on_copy: Option<OnState<State>>,
    on_collapse_change: Option<OnArg<State, bool>>,
}

/// Create a code panel showing `code`.
///
/// Monochrome and expanded by default; chain [`CodeBlockView::colored`] for
/// syntax colors, [`CodeBlockView::collapsed`] plus
/// [`CodeBlockView::on_collapse_change`] for the height morph, and
/// [`CodeBlockView::on_copy`] for the copy affordance.
pub fn code_block<State: 'static>(code: impl Into<String>) -> CodeBlockView<State> {
    CodeBlockView {
        code: code.into(),
        colored: None,
        language: "text".to_owned(),
        filename: None,
        status: CodeBlockStatus::default(),
        line_numbers: true,
        highlight_lines: Vec::new(),
        max_height: CODE_BLOCK_MAX_HEIGHT,
        collapsed: false,
        follow: false,
        on_copy: None,
        on_collapse_change: None,
    }
}

impl<State: 'static> CodeBlockView<State> {
    /// The language tag shown in the header (`language`; `"typescript"`
    /// upstream, `"text"` here since nothing is inferred).
    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// The file name shown beside the tag (`filename`).
    pub fn filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Whether the payload is still arriving (`status`).
    pub fn status(mut self, status: CodeBlockStatus) -> Self {
        self.status = status;
        self
    }

    /// Whether the line-number gutter is shown (`showLineNumbers`).
    pub fn line_numbers(mut self, line_numbers: bool) -> Self {
        self.line_numbers = line_numbers;
        self
    }

    /// The **1-based** line numbers painted with the highlight wash
    /// (`highlightLines`).
    pub fn highlight_lines(mut self, lines: impl Into<Vec<usize>>) -> Self {
        self.highlight_lines = lines.into();
        self
    }

    /// Cap the code viewport at `max_height` logical px (`maxHeight`).
    pub fn max_height(mut self, max_height: f64) -> Self {
        self.max_height = max_height.max(0.0);
        self
    }

    /// Show only [`CODE_BLOCK_COLLAPSED_LINES`] lines, morphing to and from the
    /// full height. **Controlled** — pair it with
    /// [`on_collapse_change`](Self::on_collapse_change), which is also what
    /// makes the header a toggle trigger.
    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    /// Pin the visible window to the **last** lines rather than the first — the
    /// standing effect of upstream's scroll-to-bottom while streaming (see the
    /// [module docs](self)' degradations).
    pub fn follow(mut self, follow: bool) -> Self {
        self.follow = follow;
        self
    }

    /// Supply per-line syntax spans (see the [module docs](self)).
    ///
    /// One [`CodeSpan`] list per line, in order. A list shorter than the code's
    /// own line count leaves the remaining lines monochrome, and an empty entry
    /// leaves that one line monochrome — so a partially-tokenized streaming
    /// payload renders the way upstream's does. The `code` string always
    /// decides how many lines the block has.
    pub fn colored(mut self, lines: impl Into<Vec<Vec<CodeSpan>>>) -> Self {
        self.colored = Some(lines.into());
        self
    }

    /// Report a press on the copy affordance.
    ///
    /// The component never touches a clipboard — see the [module docs](self).
    /// Wiring this is also what paints the affordance at all.
    pub fn on_copy<F: Fn(&mut State) + 'static>(mut self, on_copy: F) -> Self {
        self.on_copy = Some(Rc::new(on_copy));
        self
    }

    /// Report the collapsed state a press on the header asks for, and make the
    /// header a toggle trigger.
    pub fn on_collapse_change<F: Fn(&mut State, bool) + 'static>(mut self, on_change: F) -> Self {
        self.on_collapse_change = Some(Rc::new(on_change));
        self
    }

    /// The resolved per-line spans this view renders.
    fn resolved_lines(&self) -> Vec<Vec<CodeSpan>> {
        let mut plain = plain_spans(&self.code);
        if let Some(colored) = &self.colored {
            for (slot, supplied) in plain.iter_mut().zip(colored.iter()) {
                if !supplied.is_empty() {
                    *slot = supplied.clone();
                }
            }
        }
        plain
    }
}

/// One shaped span of one code line.
struct SpanRun {
    run: LabelRun,
    class: CodeTokenClass,
}

/// One retained code line: its shaped spans and its shaped line number.
struct LineRuns {
    spans: Vec<SpanRun>,
    number: LabelRun,
}

/// Which of the block's two affordances the roving cursor sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CodeBlockTarget {
    /// The header strip, which toggles the collapse when one is wired.
    #[default]
    Header,
    /// The copy affordance.
    Copy,
}

/// The retained widget for a [`CodeBlockView`].
pub struct CodeBlockWidget {
    lines: Vec<LineRuns>,
    line_texts: Vec<Vec<(String, CodeTokenClass)>>,
    language: LabelRun,
    language_text: String,
    filename: Option<LabelRun>,
    filename_text: Option<String>,
    status_label: LabelRun,
    status: CodeBlockStatus,
    line_numbers: bool,
    highlight_lines: Vec<usize>,
    max_height: f64,
    collapsed: bool,
    follow: bool,
    /// The `0 → 1` morph between the collapsed and expanded band heights.
    band: Lane,
    /// The copy affordance's press shrink.
    copy_press: SpringScalar,
    /// Set by a copy press, latched to a frame time on the next paint.
    copy_pending: bool,
    /// When the copy confirmation started, so it can time itself out.
    copied_since: Option<FrameTime>,
    /// The width layout resolved.
    width: f64,
    /// The natural (uncapped) height of the code body.
    natural_height: f64,
    /// The roving keyboard cursor.
    focused: CodeBlockTarget,
    /// The latched hovered affordance, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<CodeBlockTarget>,
    /// The affordance a `Down` armed.
    captured: Option<CodeBlockTarget>,
    on_copy: Option<ErasedCallback>,
    on_collapse_change: Option<ErasedArgCallback<bool>>,
}

/// Flatten the per-line spans into the `(text, class)` pairs the widget diffs
/// its shaped runs against.
fn spans_to_texts(lines: &[Vec<CodeSpan>]) -> Vec<Vec<(String, CodeTokenClass)>> {
    lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| (span.text.clone(), span.class))
                .collect()
        })
        .collect()
}

/// Build the shape-cache carriers for every line.
fn build_lines(texts: &[Vec<(String, CodeTokenClass)>]) -> Vec<LineRuns> {
    texts
        .iter()
        .enumerate()
        .map(|(index, spans)| LineRuns {
            spans: spans
                .iter()
                .map(|(text, class)| SpanRun {
                    run: LabelRun::new(text.clone()),
                    class: *class,
                })
                .collect(),
            number: LabelRun::new((index + 1).to_string()),
        })
        .collect()
}

impl<State: 'static> View<State> for CodeBlockView<State> {
    type Element = CodeBlockWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CodeBlockWidget {
        let line_texts = spans_to_texts(&self.resolved_lines());
        CodeBlockWidget {
            lines: build_lines(&line_texts),
            line_texts,
            language: LabelRun::new(self.language.to_uppercase()),
            language_text: self.language.clone(),
            filename: self.filename.as_ref().map(LabelRun::new),
            filename_text: self.filename.clone(),
            status_label: LabelRun::new(self.status.label()),
            status: self.status,
            line_numbers: self.line_numbers,
            highlight_lines: self.highlight_lines.clone(),
            max_height: self.max_height,
            collapsed: self.collapsed,
            follow: self.follow,
            band: Lane::at_rest(CODE_BLOCK_MORPH, if self.collapsed { 0.0 } else { 1.0 }),
            copy_press: SpringScalar::new(0.0, Ramp::spring(SPRING_PRESS)),
            copy_pending: false,
            copied_since: None,
            width: 0.0,
            natural_height: 0.0,
            focused: CodeBlockTarget::default(),
            hovered: None,
            captured: None,
            on_copy: self.on_copy.as_ref().map(erase_callback),
            on_collapse_change: self.on_collapse_change.as_ref().map(erase_callback_arg),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CodeBlockWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_copy = self.on_copy.as_ref().map(erase_callback);
        element.on_collapse_change = self.on_collapse_change.as_ref().map(erase_callback_arg);
        let mut flags = ChangeFlags::NONE;

        if prev.code != self.code || prev.colored != self.colored {
            let texts = spans_to_texts(&self.resolved_lines());
            if texts != element.line_texts {
                element.lines = build_lines(&texts);
                element.line_texts = texts;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        if element.language_text != self.language {
            element.language = LabelRun::new(self.language.to_uppercase());
            element.language_text = self.language.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.filename_text != self.filename {
            element.filename = self.filename.as_ref().map(LabelRun::new);
            element.filename_text = self.filename.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.status != self.status {
            element.status = self.status;
            element.status_label = LabelRun::new(self.status.label());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.line_numbers != self.line_numbers
            || element.highlight_lines != self.highlight_lines
            || element.follow != self.follow
            || element.max_height != self.max_height
        {
            element.line_numbers = self.line_numbers;
            element.highlight_lines = self.highlight_lines.clone();
            element.follow = self.follow;
            element.max_height = self.max_height;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.collapsed != self.collapsed {
            element.collapsed = self.collapsed;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Retargeted, never restarted: a rebuild re-passing the flag the lane is
        // already flying toward leaves its clock alone.
        element
            .band
            .retarget(if element.collapsed { 0.0 } else { 1.0 });
        flags
    }
}

impl CodeBlockWidget {
    /// The code body's natural height: every line plus the viewport padding.
    fn natural_body_height(&self) -> f64 {
        self.lines.len() as f64 * CODE_LINE_HEIGHT + CODE_PADDING_Y * 2.0
    }

    /// The height the body settles at when expanded — its natural height,
    /// capped.
    fn expanded_height(&self) -> f64 {
        self.natural_height.min(self.max_height)
    }

    /// The height the body settles at when collapsed.
    fn collapsed_height(&self) -> f64 {
        (CODE_BLOCK_COLLAPSED_LINES as f64 * CODE_LINE_HEIGHT + CODE_PADDING_Y * 2.0)
            .min(self.expanded_height())
    }

    /// The body's height right now, tracking the morph lane.
    fn body_height(&self) -> f64 {
        let from = self.collapsed_height();
        let to = self.expanded_height();
        (from + (to - from) * self.band.value().clamp(0.0, 1.0)).max(0.0)
    }

    /// Whether a collapse toggle is wired, which is what makes the header a
    /// trigger.
    fn collapsible(&self) -> bool {
        self.on_collapse_change.is_some()
    }

    /// Whether a copy affordance is painted (`copyable || onCopy` upstream; a
    /// port with no clipboard of its own needs the handler).
    fn copyable(&self) -> bool {
        self.on_copy.is_some()
    }

    /// The copy affordance's box, in widget-local space.
    fn copy_rect(&self) -> Option<Rect> {
        if !self.copyable() {
            return None;
        }
        let x = (self.width - CODE_BLOCK_HEADER_PADDING_X - CODE_COPY_BOX).max(0.0);
        Some(Rect::from_origin_size(
            Point::new(x, (CODE_BLOCK_HEADER_HEIGHT - CODE_COPY_BOX) / 2.0),
            Size::new(CODE_COPY_BOX, CODE_COPY_BOX),
        ))
    }

    /// The header strip's box, in widget-local space.
    fn header_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(self.width, CODE_BLOCK_HEADER_HEIGHT),
        )
    }

    /// The affordance under a widget-local `pos`, if any. The copy box wins
    /// over the header strip it sits inside.
    fn hit(&self, pos: Point) -> Option<CodeBlockTarget> {
        if let Some(rect) = self.copy_rect()
            && rect.contains(pos)
        {
            return Some(CodeBlockTarget::Copy);
        }
        if self.collapsible() && self.header_rect().contains(pos) {
            return Some(CodeBlockTarget::Header);
        }
        None
    }

    /// The first line the viewport shows — the head, or the tail when
    /// [`CodeBlockView::follow`] is set and the body is capped.
    fn first_visible_line(&self, band: f64) -> usize {
        if !self.follow {
            return 0;
        }
        let rows = ((band - CODE_PADDING_Y * 2.0) / CODE_LINE_HEIGHT)
            .floor()
            .max(0.0) as usize;
        self.lines.len().saturating_sub(rows)
    }

    /// Report the decision `target` asks for. Fires exactly one callback.
    fn activate(&mut self, ctx: &mut EventCtx, target: CodeBlockTarget) {
        match target {
            CodeBlockTarget::Copy => {
                if let Some(on_copy) = self.on_copy.as_mut() {
                    on_copy(ctx);
                    self.copy_pending = true;
                    ctx.request_redraw();
                }
            }
            CodeBlockTarget::Header => {
                let next = !self.collapsed;
                if let Some(on_change) = self.on_collapse_change.as_mut() {
                    on_change(ctx, next);
                }
            }
        }
    }

    /// The affordances the roving cursor can reach, in visual order.
    fn targets(&self) -> Vec<CodeBlockTarget> {
        let mut targets = Vec::new();
        if self.collapsible() {
            targets.push(CodeBlockTarget::Header);
        }
        if self.copyable() {
            targets.push(CodeBlockTarget::Copy);
        }
        targets
    }

    /// The next affordance `step` places from the focused one, wrapping.
    fn step_target(&self, step: isize) -> Option<CodeBlockTarget> {
        let targets = self.targets();
        if targets.is_empty() {
            return None;
        }
        let at = targets.iter().position(|t| *t == self.focused).unwrap_or(0);
        let next = (at as isize + step).rem_euclid(targets.len() as isize) as usize;
        targets.get(next).copied()
    }
}

/// Paint the check mark the complete status and the copy confirmation share,
/// inside a `box_size`-square at `origin`.
pub(crate) fn draw_check(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let mut path = BezPath::new();
    path.move_to(Point::new(box_size * 0.22, box_size * 0.52));
    path.line_to(Point::new(box_size * 0.42, box_size * 0.72));
    path.line_to(Point::new(box_size * 0.80, box_size * 0.28));
    scene.stroke_path(origin, &path, 1.5, &Brush::Solid(color));
}

/// Paint a cross, inside a `box_size`-square at `origin`.
pub(crate) fn draw_cross(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let (lo, hi) = (box_size * 0.28, box_size * 0.72);
    let mut path = BezPath::new();
    path.move_to(Point::new(lo, lo));
    path.line_to(Point::new(hi, hi));
    path.move_to(Point::new(hi, lo));
    path.line_to(Point::new(lo, hi));
    scene.stroke_path(origin, &path, 1.5, &Brush::Solid(color));
}

/// Paint the copy affordance's two-rectangle glyph.
fn draw_copy(scene: &mut dyn PaintScene, origin: Point, box_size: f64, color: Color) {
    let unit = box_size / 16.0;
    for rect in [
        Rect::new(unit * 5.0, unit * 2.0, unit * 14.0, unit * 11.0),
        Rect::new(unit * 2.0, unit * 5.0, unit * 11.0, unit * 14.0),
    ] {
        let rounded = RoundedRect::from_rect(rect, unit * 1.5);
        scene.stroke_path(
            origin,
            &Shape::to_path(&rounded, style::PATH_TOLERANCE),
            1.2,
            &Brush::Solid(color),
        );
    }
}

/// Paint a three-quarter spinner arc rotated by `angle` radians inside a
/// `box_size`-square at `origin` — Tailwind's `animate-spin` circle.
pub(crate) fn draw_spinner(
    scene: &mut dyn PaintScene,
    origin: Point,
    box_size: f64,
    angle: f64,
    color: Color,
) {
    let radius = box_size * 0.36;
    let mut path = BezPath::new();
    let steps = 18;
    let sweep = std::f64::consts::PI * 1.5;
    for step in 0..=steps {
        let theta = sweep * f64::from(step) / f64::from(steps);
        let point = Point::new(radius * theta.cos(), radius * theta.sin());
        if step == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    let centre = Point::new(origin.x + box_size / 2.0, origin.y + box_size / 2.0);
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    scene.stroke_path(Point::ZERO, &path, 1.5, &Brush::Solid(color));
    scene.pop_transform();
}

/// The angle a spinner has turned to at `now`, or zero under reduced motion.
pub(crate) fn spinner_angle(now: FrameTime, reduce_motion: bool) -> f64 {
    if reduce_motion {
        return 0.0;
    }
    now.as_secs_f64() / CODE_SPINNER_PERIOD.as_secs_f64() * std::f64::consts::TAU
}

/// Paint a page outline with a folded corner — the `FileCode2` glyph's
/// silhouette.
pub(crate) fn draw_file_glyph(
    scene: &mut dyn PaintScene,
    origin: Point,
    box_size: f64,
    color: Color,
) {
    let unit = box_size / 16.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(unit * 4.0, unit * 1.5));
    path.line_to(Point::new(unit * 10.0, unit * 1.5));
    path.line_to(Point::new(unit * 13.0, unit * 5.0));
    path.line_to(Point::new(unit * 13.0, unit * 14.5));
    path.line_to(Point::new(unit * 4.0, unit * 14.5));
    path.close_path();
    scene.stroke_path(origin, &path, 1.2, &Brush::Solid(color));
}

/// Paint a chevron pointing down, rotated `angle` radians about `centre`.
pub(crate) fn draw_chevron(
    scene: &mut dyn PaintScene,
    centre: Point,
    box_size: f64,
    angle: f64,
    color: Color,
) {
    let arm = box_size * 0.18;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, -arm * 0.6));
    path.line_to(Point::new(0.0, arm * 0.6));
    path.line_to(Point::new(arm, -arm * 0.6));
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    scene.stroke_path(Point::ZERO, &path, 1.75, &Brush::Solid(color));
    scene.pop_transform();
}

impl Widget for CodeBlockWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        self.width = bc.max().width;
        let code = code_style(CODE_TEXT_SIZE);
        for line in &mut self.lines {
            for span in &mut line.spans {
                span.run.layout(ctx, &code);
            }
            if self.line_numbers {
                line.number.layout(ctx, &code);
            }
        }
        self.language.layout(ctx, &chrome_style(CODE_LANGUAGE_SIZE));
        self.status_label
            .layout(ctx, &chrome_style(CODE_LANGUAGE_SIZE));
        if let Some(filename) = &mut self.filename {
            filename.layout(ctx, &code);
        }

        self.natural_height = self.natural_body_height();
        bc.constrain(Size::new(
            self.width,
            CODE_BLOCK_HEADER_HEIGHT + self.body_height(),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let palette = code_palette(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let tokens = BeuiTokens::resolve(theme);
        let ring = with_alpha(
            BeuiTokens::resolve_ring(None, theme),
            style::FOCUS_RING_OPACITY,
        );
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let before = self.band.value();
        let moving = if reduce_motion {
            self.band.snap();
            false
        } else {
            self.band.advance(now)
        };
        let press = self.copy_press.advance(now);

        // The copy confirmation is a latch timed off the frame clock: the event
        // pass that set it carries none.
        if self.copy_pending {
            self.copy_pending = false;
            self.copied_since = Some(now);
        }
        let copied = match self.copied_since {
            Some(since) if now.saturating_sub(since) < CODE_COPY_FEEDBACK => true,
            Some(_) => {
                self.copied_since = None;
                false
            }
            None => false,
        };

        scene.fill_rounded_rect(origin, size, CODE_BLOCK_RADIUS, palette.surface);
        scene.push_clip_rounded(origin, size, CODE_BLOCK_RADIUS);

        // ---- header ---------------------------------------------------------
        let mut x = origin.x + CODE_BLOCK_HEADER_PADDING_X;
        let icon_y = origin.y + (CODE_BLOCK_HEADER_HEIGHT - style::ICON_SIZE) / 2.0;
        draw_file_glyph(
            scene,
            Point::new(x, icon_y),
            style::ICON_SIZE,
            with_alpha(palette.comment, 0.7),
        );
        x += style::ICON_SIZE + CODE_BLOCK_HEADER_GAP;

        if let Some(filename) = &self.filename {
            let measured = filename.size();
            filename.paint(
                Point::new(
                    x,
                    origin.y + (CODE_BLOCK_HEADER_HEIGHT - measured.height) / 2.0,
                ),
                with_alpha(palette.plain, CODE_FILENAME_ALPHA),
                scene,
            );
            x += measured.width + CODE_BLOCK_HEADER_GAP;
        }

        let language_size = self.language.size();
        self.language.paint(
            Point::new(
                x,
                origin.y + (CODE_BLOCK_HEADER_HEIGHT - language_size.height) / 2.0,
            ),
            with_alpha(palette.comment, CODE_LANGUAGE_ALPHA),
            scene,
        );

        // The status cluster is right-aligned, ahead of the copy affordance.
        let status_ink = if self.status.is_streaming() {
            palette.function
        } else {
            tokens.success
        };
        let status_size = self.status_label.size();
        let copy_reserve = if self.copyable() {
            CODE_COPY_BOX + CODE_BLOCK_HEADER_GAP
        } else {
            0.0
        };
        let status_x = (origin.x + size.width
            - CODE_BLOCK_HEADER_PADDING_X
            - copy_reserve
            - status_size.width)
            .max(x);
        self.status_label.paint(
            Point::new(
                status_x,
                origin.y + (CODE_BLOCK_HEADER_HEIGHT - status_size.height) / 2.0,
            ),
            status_ink,
            scene,
        );
        let mark_box = style::ICON_SIZE * 0.75;
        let mark_at = Point::new(
            (status_x - mark_box - style::GAP_SM / 2.0).max(x),
            origin.y + (CODE_BLOCK_HEADER_HEIGHT - mark_box) / 2.0,
        );
        if self.status.is_streaming() {
            draw_spinner(
                scene,
                mark_at,
                mark_box,
                spinner_angle(now, reduce_motion),
                status_ink,
            );
            if !reduce_motion {
                ctx.request_frame_paced();
            }
        } else {
            draw_check(scene, mark_at, mark_box, status_ink);
        }

        if let Some(rect) = self.copy_rect() {
            let at = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let hovered = self.hovered == Some(CodeBlockTarget::Copy);
            if hovered {
                scene.fill_rounded_rect(
                    at,
                    rect.size(),
                    rect.width() / 2.0,
                    with_alpha(palette.plain, style::HOVER_WASH_ALPHA),
                );
            }
            let scale = press_scale(CODE_COPY_PRESS_SCALE, press);
            let inset = CODE_COPY_BOX * (1.0 - scale) / 2.0;
            let glyph_at = Point::new(at.x + inset, at.y + inset);
            let glyph_box = CODE_COPY_BOX * scale;
            if copied {
                draw_check(scene, glyph_at, glyph_box, tokens.success);
            } else {
                draw_copy(
                    scene,
                    glyph_at,
                    glyph_box,
                    if hovered {
                        palette.plain
                    } else {
                        palette.comment
                    },
                );
            }
            if self.focused == CodeBlockTarget::Copy && ctx.has_focus() {
                draw_focus_ring(scene, at, rect.size(), rect.width() / 2.0, 0.0, ring);
            }
        }

        // ---- code body ------------------------------------------------------
        let band = self.body_height();
        let body_top = origin.y + CODE_BLOCK_HEADER_HEIGHT;
        if band > 0.5 {
            scene.fill_rect(
                Point::new(origin.x, body_top),
                Size::new(size.width, style::BORDER_WIDTH),
                with_alpha(palette.plain, CODE_HAIRLINE_ALPHA),
            );
            scene.push_clip(Point::new(origin.x, body_top), Size::new(size.width, band));
            let first = self.first_visible_line(band);
            let code_x = if self.line_numbers {
                origin.x + CODE_GUTTER_WIDTH + CODE_GUTTER_GAP
            } else {
                origin.x + CODE_PADDING_X
            };
            for (index, line) in self.lines.iter().enumerate().skip(first) {
                let row_top = body_top + CODE_PADDING_Y + (index - first) as f64 * CODE_LINE_HEIGHT;
                if row_top >= body_top + band {
                    break;
                }
                if self.highlight_lines.contains(&(index + 1)) {
                    scene.fill_rect(
                        Point::new(origin.x, row_top),
                        Size::new(size.width, CODE_LINE_HEIGHT),
                        with_alpha(palette.function, CODE_HIGHLIGHT_ALPHA),
                    );
                }
                if self.line_numbers {
                    let measured = line.number.size();
                    line.number.paint(
                        Point::new(
                            origin.x + CODE_GUTTER_WIDTH - CODE_GUTTER_PADDING - measured.width,
                            row_top + (CODE_LINE_HEIGHT - measured.height) / 2.0,
                        ),
                        with_alpha(palette.comment, CODE_GUTTER_ALPHA),
                        scene,
                    );
                }
                let mut span_x = code_x;
                for span in &line.spans {
                    let measured = span.run.size();
                    span.run.paint(
                        Point::new(span_x, row_top + (CODE_LINE_HEIGHT - measured.height) / 2.0),
                        palette.ink(span.class),
                        scene,
                    );
                    span_x += measured.width;
                }
            }
            scene.pop_clip();
        }
        scene.pop_clip();

        // The band height is computed in `layout` from the lane, so a bare frame
        // request would let the morph freeze on the intra-frame layout skip. Ask
        // for layout while it moves, and once more on the frame the value
        // actually changed — which covers the landing frame and the
        // `reduce_motion` snap.
        if moving || self.band.value() != before {
            ctx.request_layout();
        }
        if self.copy_press.is_animating() || copied {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            return EventResult::Ignored;
        }
        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowRight) => Some(1),
                    Key::Named(NamedKey::ArrowLeft) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    let Some(next) = self.step_target(step) else {
                        return EventResult::Ignored;
                    };
                    self.focused = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) {
                    let target = self.focused;
                    if !self.targets().contains(&target) {
                        return EventResult::Ignored;
                    }
                    self.activate(ctx, target);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) || !inside(p.position, ctx.size()) {
                        return EventResult::Ignored;
                    }
                    let Some(target) = self.hit(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(target);
                    self.focused = target;
                    if target == CodeBlockTarget::Copy {
                        self.copy_press.set_target(1.0);
                    }
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_some() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        return EventResult::Handled;
                    }
                    let over = self.hit(p.position);
                    if over.is_some() {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                    if self.hovered != over {
                        self.hovered = over;
                        ctx.request_redraw();
                    }
                    EventResult::Ignored
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    self.copy_press.set_target(0.0);
                    if self.hit(p.position) == Some(armed) {
                        self.activate(ctx, armed);
                    }
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    self.copy_press.set_target(0.0);
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = match &self.filename_text {
            Some(name) => format!("{name} ({}) — {}", self.language_text, self.status.label()),
            None => format!("{} code — {}", self.language_text, self.status.label()),
        };
        ctx.push_node(Role::Group, |node| {
            node.set_label(label.as_str());
            node.set_expanded(!self.collapsed);
            if self.collapsible() {
                node.add_action(Action::Click);
            }
        });
        if self.copyable() {
            ctx.push_node(Role::Button, |node| {
                node.set_label("Copy code");
                node.add_action(Action::Click);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{CornerRadii, KeyEvent, Modifiers, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rounded: Vec<(Point, Size, Color)>,
        clips: Vec<(Point, Size)>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
        strokes: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, _r: f64, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, _r: CornerRadii, c: Color) {
            self.rounded.push((o, s, c));
        }
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_clip_rounded(&mut self, o: Point, s: Size, _r: f64) {
            self.clips.push((o, s));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {
            self.strokes += 1;
        }
        fn push_transform(&mut self, t: Affine) {
            self.transforms.push(t);
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Copied {
        copies: u32,
        collapse: Option<bool>,
        collapse_calls: u32,
    }

    const SAMPLE: &str =
        "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n    let y = 2;\n    let z = 3;\n}";

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn view(collapsed: bool) -> CodeBlockView<Copied> {
        code_block::<Copied>(SAMPLE)
            .language("rust")
            .filename("main.rs")
            .collapsed(collapsed)
            .on_copy(|s: &mut Copied| s.copies += 1)
            .on_collapse_change(|s: &mut Copied, next: bool| {
                s.collapse = Some(next);
                s.collapse_calls += 1;
            })
    }

    fn build(v: &CodeBlockView<Copied>) -> CodeBlockWidget {
        let mut counter = 0u64;
        View::<Copied>::build(v, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut CodeBlockWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(420.0, 900.0)),
        )
    }

    fn laid_out(collapsed: bool) -> (CodeBlockWidget, Size) {
        let mut w = build(&view(collapsed));
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut CodeBlockWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn rebuild(w: &mut CodeBlockWidget, from: bool, to: bool) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Copied>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut CodeBlockWidget, size: Size, event: &InputEvent, state: &mut Copied) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn press(w: &mut CodeBlockWidget, size: Size, at: Point, state: &mut Copied) {
        dispatch(w, size, &pointer(PointerPhase::Down, at), state);
        dispatch(w, size, &pointer(PointerPhase::Up, at), state);
    }

    fn key(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    /// The model: a plain string becomes one monochrome span per line, and a
    /// caller's own tokens replace a line's spans without changing the line
    /// count — including for a partially-tokenized streaming payload.
    #[test]
    fn plain_code_is_monochrome_and_supplied_spans_replace_a_line() {
        let lines = plain_spans("a\nb\nc");
        assert_eq!(lines.len(), 3);
        assert!(
            lines
                .iter()
                .all(|l| l.len() == 1 && l[0].class == CodeTokenClass::Plain)
        );

        let colored = code_block::<Copied>("a\nb\nc").colored(vec![
            vec![
                code_span("a", CodeTokenClass::Keyword),
                code_span("!", CodeTokenClass::Punctuation),
            ],
            Vec::new(),
        ]);
        let resolved = colored.resolved_lines();
        assert_eq!(resolved.len(), 3, "the code string decides the line count");
        assert_eq!(resolved[0].len(), 2);
        assert_eq!(resolved[0][0].class, CodeTokenClass::Keyword);
        assert_eq!(
            resolved[1][0].class,
            CodeTokenClass::Plain,
            "an empty supplied line leaves the plain fallback"
        );
        assert_eq!(resolved[2][0].text, "c");
    }

    /// Every class resolves to an ink, the palette falls back to the vendored
    /// light table unthemed, and a themed pass moves off it.
    #[test]
    fn the_code_palette_resolves_every_class_themed_and_unthemed() {
        let unthemed = code_palette(None);
        let tokens = BeuiTokens::beui();
        assert_eq!(unthemed.keyword, tokens.violet);
        assert_eq!(unthemed.string, tokens.success);
        assert_eq!(unthemed.number, tokens.warning);
        assert_eq!(unthemed.type_name, tokens.neon);
        assert_eq!(unthemed.plain.components[3], CODE_PLAIN_ALPHA);
        assert_eq!(unthemed.ink(CodeTokenClass::Comment), unthemed.comment);
        assert_eq!(unthemed.ink(CodeTokenClass::Plain), unthemed.plain);
        assert_eq!(unthemed.ink(CodeTokenClass::Function), unthemed.function);

        // The unthemed fallback *is* the light table, so a light-themed pass
        // matches it and a dark one moves off it.
        let light = code_palette(Some(&crate::theme()));
        assert_eq!(light.plain, unthemed.plain);
        let dark = code_palette(Some(
            &crate::theme().with_brightness(frust::Brightness::Dark),
        ));
        assert_ne!(dark.plain, unthemed.plain);
        assert_ne!(dark.surface, unthemed.surface);
        assert_eq!(dark.keyword, tokens.violet, "brand hues are role-free");
    }

    /// The status word follows the streaming flag.
    #[test]
    fn the_status_reads_writing_while_streaming_and_ready_when_done() {
        assert_eq!(CodeBlockStatus::Streaming.label(), "Writing");
        assert_eq!(CodeBlockStatus::Complete.label(), "Ready");
        assert!(CodeBlockStatus::Streaming.is_streaming());
        assert!(!CodeBlockStatus::default().is_streaming());
    }

    /// The whole chrome paints: the surface, the header glyph and labels, the
    /// clipped code band, one line number per visible row.
    #[test]
    fn an_expanded_block_paints_its_surface_header_and_clipped_band() {
        let (mut w, size) = laid_out(false);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert!(!rec.rounded.is_empty(), "the code surface is filled");
        assert_eq!(rec.rounded[0].2, code_palette(None).surface);
        assert_eq!(
            rec.clips.len(),
            2,
            "the panel round-clip plus the code band"
        );
        assert!(rec.clips[1].1.height > 0.0);
        assert!(!rec.rects.is_empty(), "the header hairline");
        // filename + language + status + 6 line numbers + 6 code lines.
        assert!(rec.inks.len() >= 15, "painted {} runs", rec.inks.len());
        assert!(rec.strokes >= 3, "file glyph, check and copy glyph");
    }

    /// A highlighted line paints its wash and an un-highlighted one does not.
    #[test]
    fn a_highlighted_line_paints_the_accent_wash() {
        let mut plain = build(&code_block::<Copied>(SAMPLE));
        let size = layout(&mut plain);
        let (bare, _, _) = paint_at(&mut plain, size, None, 0.0);

        let mut lit = build(&code_block::<Copied>(SAMPLE).highlight_lines(vec![2usize]));
        let size = layout(&mut lit);
        let (rec, _, _) = paint_at(&mut lit, size, None, 0.0);
        assert_eq!(
            rec.rects.len(),
            bare.rects.len() + 1,
            "exactly one extra wash"
        );
        let wash = code_palette(None).function;
        assert!(
            rec.rects
                .iter()
                .any(|(_, _, c)| c.components[..3] == wash.components[..3]
                    && c.components[3] == CODE_HIGHLIGHT_ALPHA)
        );
    }

    /// Collapsing morphs the widget's own height between the collapsed band and
    /// the full one, and every moving frame asks for relayout — the layout-skip
    /// hazard a lane-driven height carries.
    #[test]
    fn collapsing_morphs_the_height_and_asks_for_relayout() {
        let (mut w, size) = laid_out(false);
        let expanded = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.band.value(), 1.0);

        rebuild(&mut w, false, true);
        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "a collapsing block must ask for relayout");

        let mut shortest = expanded;
        for step in 1..=40 {
            paint_at(&mut w, size, None, 100.0 + f64::from(step) * 25.0);
            shortest = shortest.min(layout(&mut w).height);
        }
        assert!(shortest < expanded, "the block never shrank");
        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(w.band.value(), 0.0);
        let collapsed = layout(&mut w).height;
        assert_eq!(
            collapsed,
            CODE_BLOCK_HEADER_HEIGHT + w.collapsed_height(),
            "a settled collapse is the header plus the short band"
        );
        let (_, _, still) = paint_at(&mut w, size, None, 5_100.0);
        assert!(!still, "a settled block asks for no more layout");
    }

    /// Re-passing the collapse flag already being flown toward must not restart
    /// the morph — the retarget-never-restart rule.
    #[test]
    fn a_redundant_rebuild_does_not_restart_the_morph() {
        let (mut w, size) = laid_out(false);
        paint_at(&mut w, size, None, 0.0);
        rebuild(&mut w, false, true);
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 160.0);
        let mid = w.band.value();

        rebuild(&mut w, true, true);
        paint_at(&mut w, size, None, 200.0);
        assert!(
            w.band.value() < mid,
            "the clock kept running across the redundant rebuild: {mid} -> {}",
            w.band.value()
        );
    }

    /// `reduce_motion` lands the morph on the frame it is painted and still
    /// publishes the new height.
    #[test]
    fn reduce_motion_lands_the_collapse_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(false);
        paint_at(&mut w, size, Some(&theme), 0.0);
        rebuild(&mut w, false, true);
        let (_, _, needs_layout) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(needs_layout, "the snap still publishes its new height");
        assert_eq!(w.band.value(), 0.0);
    }

    /// A press on the copy affordance reports once, and the confirmation stays
    /// up for its window then clears itself off the frame clock.
    #[test]
    fn copying_reports_once_and_the_confirmation_times_itself_out() {
        let (mut w, size) = laid_out(false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Copied::default();
        let at = w.copy_rect().unwrap().center();
        press(&mut w, size, at, &mut state);
        assert_eq!(state.copies, 1, "exactly one copy per press");

        paint_at(&mut w, size, None, 10.0);
        assert!(w.copied_since.is_some(), "the confirmation latched");
        let window = CODE_COPY_FEEDBACK.as_millis() as f64;
        paint_at(&mut w, size, None, 10.0 + window / 2.0);
        assert!(w.copied_since.is_some(), "and stays up inside its window");
        paint_at(&mut w, size, None, 10.0 + window + 1.0);
        assert!(w.copied_since.is_none(), "then clears itself");
    }

    /// A press that leaves the affordance before release fires nothing — the
    /// up-inside rule.
    #[test]
    fn a_press_released_outside_the_copy_box_reports_nothing() {
        let (mut w, size) = laid_out(false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Copied::default();
        let at = w.copy_rect().unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, Point::new(at.x, at.y + 200.0)),
            &mut state,
        );
        assert_eq!(state.copies, 0);
    }

    /// The header reports the collapse it wants and never writes its own — the
    /// controlled-component rule.
    #[test]
    fn the_header_reports_the_requested_collapse_and_never_writes_it() {
        let (mut w, size) = laid_out(false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Copied::default();
        press(
            &mut w,
            size,
            Point::new(60.0, CODE_BLOCK_HEADER_HEIGHT / 2.0),
            &mut state,
        );
        assert_eq!(state.collapse, Some(true));
        assert_eq!(state.collapse_calls, 1, "exactly once per press");
        assert!(!w.collapsed, "the widget never writes its own flag");
    }

    /// Keyboard: the arrows move the roving cursor between the header and the
    /// copy affordance, and the activation keys fire the one under it.
    #[test]
    fn the_arrows_rove_and_the_activation_keys_fire_the_focused_affordance() {
        let (mut w, size) = laid_out(false);
        paint_at(&mut w, size, None, 0.0);
        let mut state = Copied::default();
        assert_eq!(w.focused, CodeBlockTarget::Header);
        dispatch(&mut w, size, &key(NamedKey::ArrowRight), &mut state);
        assert_eq!(w.focused, CodeBlockTarget::Copy);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.copies, 1);
        dispatch(&mut w, size, &key(NamedKey::ArrowLeft), &mut state);
        assert_eq!(w.focused, CodeBlockTarget::Header);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.collapse, Some(true));
    }

    /// A streaming block turns its spinner off a paced tick, freezes it under
    /// reduced motion, and follows the tail when asked.
    #[test]
    fn a_streaming_block_paces_its_spinner_and_a_following_one_shows_the_tail() {
        let mut w = build(&code_block::<Copied>(SAMPLE).status(CodeBlockStatus::Streaming));
        let size = layout(&mut w);
        let (rec, needs_frame, _) = paint_at(&mut w, size, None, 0.0);
        assert!(needs_frame, "the spinner asks for its next tick");
        assert_eq!(rec.transforms.len(), 1, "one rotated spinner");
        assert!(spinner_angle(ft_ms(500.0), false) > 0.0);
        assert_eq!(spinner_angle(ft_ms(500.0), true), 0.0);

        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (_, needs_frame, _) = paint_at(&mut w, size, Some(&theme), 20.0);
        assert!(!needs_frame, "reduced motion freezes the spinner");

        let mut following = build(
            &code_block::<Copied>(SAMPLE)
                .status(CodeBlockStatus::Streaming)
                .max_height(CODE_LINE_HEIGHT * 2.0 + CODE_PADDING_Y * 2.0)
                .follow(true),
        );
        let size = layout(&mut following);
        paint_at(&mut following, size, None, 0.0);
        let band = following.body_height();
        assert!(
            following.first_visible_line(band) > 0,
            "a following block shows the tail, not the head"
        );
    }

    /// A block with no callbacks has no affordances at all: nothing to hit,
    /// nothing to rove between, nothing to fire.
    #[test]
    fn a_block_with_no_callbacks_is_inert() {
        let mut w = build(&code_block::<Copied>(SAMPLE));
        let size = layout(&mut w);
        paint_at(&mut w, size, None, 0.0);
        assert!(w.copy_rect().is_none());
        assert!(!w.collapsible());
        assert_eq!(w.hit(Point::new(10.0, 10.0)), None);
        assert!(w.targets().is_empty());
        let mut state = Copied::default();
        press(&mut w, size, Point::new(10.0, 10.0), &mut state);
        dispatch(&mut w, size, &key(NamedKey::Enter), &mut state);
        assert_eq!(state.copies, 0);
        assert_eq!(state.collapse_calls, 0);
    }
}
