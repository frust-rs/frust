//! [`markdown`]: assistant markdown rendered in Glyph styling from the block
//! model of [`super::markdown_parse`].
//!
//! # Contract
//!
//! - **Controlled and non-reactive.** The caller owns the source text and
//!   re-builds the view as it grows; nothing here opens a URL, loads an image
//!   or interprets HTML. A safe link is reported through `.on_link(..)`; an
//!   unsafe one, or one whose label is an http(s) URL of another host, is
//!   plain text then its URL in parentheses. Hidden characters in a label or
//!   a shown URL read as `?`.
//! - **One widget per block.** Every top-level block is its own child, keyed
//!   by position; a block whose model (and streaming role) is unchanged keeps
//!   its widget across a rebuild: no re-render, re-shape or layout.
//! - **Parsing.** Only a changed source is parsed, and each change re-parses
//!   the whole of it: coalesce streamed chunks to one update per frame. A
//!   source over [`MARKDOWN_MAX_SOURCE_BYTES`] (256 KiB) is plain wrapped
//!   text, never parsed.
//! - **Streaming.** `.streaming(true)` draws a dots row under the last block,
//!   marks an unclosed code fence as streaming and lets a trailing table keep
//!   its column widths.
//! - **Selection.** With `.selectable(true)`, the default, every paragraph,
//!   heading, list item and quote paragraph is its own selectable
//!   `rich_text`: a selection stays inside one block. Table cells are not.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use frust::accesskit::Role;
use frust::authoring::text::{FontWeight, OverflowWrap};
use frust::authoring::{
    AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Rect, SemanticsCtx, Size,
    ThemeTextColor, ThemeTextType, Toggled, View, Widget, any, build_child, rebuild_child,
    route_event, teardown_child,
};
use frust::{
    FlexView, RichTextView, SpanView, SurfaceRole, Theme, column, rich_text, semantics, span,
};

use super::code_block::code_block;
use super::markdown_parse::{MdBlock, MdInline, MdListItem, parse_markdown, plain_text};
use super::style;
use super::table::table;

/// Body text size: the Glyph `body` token.
const BODY_SIZE: f32 = style::BODY_FONT_SIZE;
/// Heading sizes for levels 1, 2 and 3 to 6: the Glyph `heading`, `title`
/// and `body` tokens.
const HEADING_SIZES: [f32; 3] = [20.0, 15.0, 13.0];
/// Height of the streaming indicator row.
const INDICATOR_H: f64 = 12.0;
/// Side of one streaming indicator dot.
const DOT: f64 = 4.0;
/// Left inset of a block quote's content (the bar sits inside it).
const QUOTE_INDENT: f64 = 14.0;
/// Width of a block quote's bar.
const QUOTE_BAR_W: f64 = 3.0;
/// Narrowest list marker column.
const LIST_INDENT: f64 = 18.0;
/// Space between a list marker and the item text.
const MARKER_GAP: f64 = style::PAD_SMALL;
/// Side of a task-list checkbox.
const CHECK_SIDE: f64 = 11.0;
/// Space above a task-list checkbox, to sit it on the first text line.
const CHECK_TOP: f64 = 3.0;
/// Height of a thematic break's row.
const RULE_H: f64 = style::BLOCK_GAP;
/// Bullet of a top-level list item (U+2022, present in the mono face).
const BULLET: &str = "\u{2022}";
/// Bytes per paragraph of a source shown as plain text.
const PLAIN_CHUNK_BYTES: usize = 4 * 1024;

/// The largest source [`markdown`] parses: a longer one is shown as plain
/// wrapped text, unparsed. Every change re-parses the whole source, and the
/// parse and the block diff grow with it; 256 KiB is far beyond an assistant
/// reply (a few KiB) yet keeps one re-parse well inside a frame, and matches
/// the order of the diff view's own per-side guard.
pub const MARKDOWN_MAX_SOURCE_BYTES: usize = 256 * 1024;

type OnText<State> = Rc<dyn Fn(&mut State, &str)>;

/// The view's callbacks, shared by every block a widget renders: a block
/// kept across rebuilds still reaches the latest closures.
struct Hooks<State: 'static> {
    on_link: RefCell<Option<OnText<State>>>,
    on_copy: RefCell<Option<OnText<State>>>,
}

impl<State: 'static> Hooks<State> {
    fn link(&self, state: &mut State, url: &str) {
        let callback = self.on_link.borrow().clone();
        if let Some(callback) = callback {
            callback(state, url);
        }
    }

    fn copy(&self, state: &mut State, code: &str) {
        let callback = self.on_copy.borrow().clone();
        if let Some(callback) = callback {
            callback(state, code);
        }
    }
}

/// A declarative markdown block. See the [module docs](self).
pub struct MarkdownView<State: 'static> {
    source: String,
    streaming: bool,
    selectable: bool,
    on_link: Option<OnText<State>>,
    on_copy: Option<OnText<State>>,
}

impl<State: 'static> std::fmt::Debug for MarkdownView<State> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownView")
            .field("source", &self.source)
            .field("streaming", &self.streaming)
            .field("selectable", &self.selectable)
            .finish_non_exhaustive()
    }
}

/// Render `source` as markdown in Glyph styling. The caller owns the source
/// and passes the whole current text on every build.
pub fn markdown<State: 'static>(source: impl Into<String>) -> MarkdownView<State> {
    MarkdownView {
        source: source.into(),
        streaming: false,
        selectable: true,
        on_link: None,
        on_copy: None,
    }
}

impl<State: 'static> MarkdownView<State> {
    /// Mark the text as still arriving: see the module docs' *Streaming*.
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// Run `on_link` with the app state and the exact URL of a safe link the
    /// user taps. Nothing is opened here. The URL is attacker-controlled and
    /// may differ from the link's visible text: show or confirm the
    /// destination before opening it.
    pub fn on_link(mut self, on_link: impl Fn(&mut State, &str) + 'static) -> Self {
        self.on_link = Some(Rc::new(on_link));
        self
    }

    /// Let the user select text, one block at a time (default `true`).
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Run `on_copy` with the app state and the code when a code block's copy
    /// control is pressed (the clipboard is written first).
    pub fn on_copy(mut self, on_copy: impl Fn(&mut State, &str) + 'static) -> Self {
        self.on_copy = Some(Rc::new(on_copy));
        self
    }

    /// What every block of `blocks` is rendered with.
    fn params(&self, blocks: &[MdBlock]) -> Vec<Params> {
        let last = blocks.len().checked_sub(1);
        blocks
            .iter()
            .enumerate()
            .map(|(index, block)| Params {
                selectable: self.selectable,
                links: self.on_link.is_some(),
                live: self.streaming && Some(index) == last && is_live(block),
            })
            .collect()
    }

    fn install(&self, hooks: &Hooks<State>) {
        hooks.on_link.replace(self.on_link.clone());
        hooks.on_copy.replace(self.on_copy.clone());
    }
}

/// Whether a block renders differently while it is the streaming tail: a
/// table keeps its widths, an unclosed fence is marked streaming.
fn is_live(block: &MdBlock) -> bool {
    match block {
        MdBlock::Table { .. } => true,
        MdBlock::CodeBlock { closed, .. } => !closed,
        MdBlock::List { items, .. } => items.iter().any(|item| item.blocks.iter().any(is_live)),
        MdBlock::BlockQuote(blocks) => blocks.iter().any(is_live),
        _ => false,
    }
}

fn has_safe_link(inlines: &[MdInline]) -> bool {
    inlines.iter().any(|inline| match inline {
        MdInline::Link {
            url,
            safe: true,
            content,
        } if tappable(url, content) => true,
        MdInline::Link { content, .. }
        | MdInline::Emphasis(content)
        | MdInline::Strong(content)
        | MdInline::Strikethrough(content) => has_safe_link(content),
        _ => false,
    })
}

/// An unsafe URL as shown: control, bidi and zero-width characters become
/// `?` (a URL has no line breaks or tabs to keep).
fn shown_url(url: &str) -> String {
    style::display_text(url)
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// The host of an `http(s)://` URL, lowercased: `None` for anything else.
fn http_host(text: &str) -> Option<String> {
    let text = text.trim();
    let rest = ["http://", "https://"].iter().find_map(|scheme| {
        let head = text.get(..scheme.len())?;
        head.eq_ignore_ascii_case(scheme)
            .then(|| &text[scheme.len()..])
    })?;
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => host_port.split(':').next().unwrap_or_default(),
    };
    let host = host.trim_end_matches('.').to_lowercase();
    (!host.is_empty()).then_some(host)
}

/// Whether a safe link may be tapped: unless its label itself reads as an
/// http(s) URL naming another host than the destination, which would show
/// the user one site and open another.
fn tappable(url: &str, content: &[MdInline]) -> bool {
    let mut label = String::new();
    plain_text(content, &mut label);
    http_host(&label).is_none_or(|shown| http_host(url) == Some(shown))
}

/// Everything a top-level block's views depend on besides its model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Params {
    selectable: bool,
    links: bool,
    live: bool,
}

/// The paragraph style a run of inlines starts from.
#[derive(Clone, Copy)]
struct Base {
    size: f32,
    family: ThemeTextType,
    bold: bool,
    role: ThemeTextColor,
}

const BODY: Base = Base {
    size: BODY_SIZE,
    family: ThemeTextType::BodyLarge,
    bold: false,
    role: ThemeTextColor::OnSurface,
};

fn heading_base(level: u8) -> Base {
    let (size, family) = match level {
        1 => (HEADING_SIZES[0], ThemeTextType::HeadlineLarge),
        2 => (HEADING_SIZES[1], ThemeTextType::TitleLarge),
        _ => (HEADING_SIZES[2], ThemeTextType::BodyLarge),
    };
    Base {
        size,
        family,
        bold: true,
        role: ThemeTextColor::OnSurface,
    }
}

/// Inline styling inherited from enclosing spans.
#[derive(Clone, Copy, Default)]
struct Look {
    italic: bool,
    bold: bool,
    strike: bool,
    /// Inside a link's label: hidden characters are shown as `?`.
    label: bool,
}

fn styled(
    text: impl Into<String>,
    base: Base,
    look: Look,
    link: Option<&str>,
    code: bool,
) -> SpanView {
    let weight = if base.bold || look.bold {
        FontWeight::BOLD
    } else {
        FontWeight::REGULAR
    };
    let text: String = text.into();
    let text = if look.label {
        style::display_text(&text)
    } else {
        text
    };
    let mut s = span(text).size(base.size).weight(weight);
    s = if code {
        s.themed_mono_family()
            .themed_background(SurfaceRole::SurfaceContainerHighest)
    } else {
        s.themed_family(base.family)
    };
    s = match link {
        Some(url) => s.themed_role(style::LINK_ROLE).underline().link(url),
        None => s.themed_role(base.role),
    };
    if look.italic {
        s = s.italic();
    }
    if look.strike {
        s = s.line_through();
    }
    s
}

fn push_spans(
    inlines: &[MdInline],
    base: Base,
    look: Look,
    link: Option<&str>,
    out: &mut Vec<SpanView>,
) {
    for inline in inlines {
        match inline {
            MdInline::Text(text) => out.push(styled(text.as_str(), base, look, link, false)),
            MdInline::Code(code) => out.push(styled(code.as_str(), base, look, link, true)),
            MdInline::Emphasis(content) => {
                let look = Look {
                    italic: true,
                    ..look
                };
                push_spans(content, base, look, link, out);
            }
            MdInline::Strong(content) => {
                let look = Look { bold: true, ..look };
                push_spans(content, base, look, link, out);
            }
            MdInline::Strikethrough(content) => {
                let look = Look {
                    strike: true,
                    ..look
                };
                push_spans(content, base, look, link, out);
            }
            MdInline::Link {
                url,
                safe: true,
                content,
            } if tappable(url, content) => {
                let look = Look {
                    label: true,
                    ..look
                };
                push_spans(content, base, look, Some(url), out);
            }
            MdInline::Link { url, content, .. } => {
                let look = Look {
                    label: true,
                    ..look
                };
                push_spans(content, base, look, link, out);
                let muted = Base {
                    role: ThemeTextColor::OnSurfaceVariant,
                    ..base
                };
                let suffix = format!(" ({})", shown_url(url));
                out.push(styled(suffix, muted, look, link, false));
            }
            MdInline::SoftBreak => out.push(styled(" ", base, look, link, false)),
            MdInline::HardBreak => out.push(styled("\n", base, look, link, false)),
        }
    }
}

/// Turns blocks into views under one set of [`Params`].
struct Render<'a, State: 'static> {
    hooks: &'a Rc<Hooks<State>>,
    params: Params,
    /// List nesting depth, for the bullet shape.
    depth: usize,
}

impl<State: 'static> Clone for Render<'_, State> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<State: 'static> Copy for Render<'_, State> {}

impl<'a, State: 'static> Render<'a, State> {
    fn new(hooks: &'a Rc<Hooks<State>>, params: Params) -> Self {
        Self {
            hooks,
            params,
            depth: 0,
        }
    }

    fn text(self, inlines: &[MdInline], base: Base, selectable: bool) -> RichTextView {
        let mut spans = Vec::new();
        push_spans(inlines, base, Look::default(), None, &mut spans);
        let mut view = rich_text(spans)
            .overflow_wrap(OverflowWrap::Anywhere)
            .selectable(selectable);
        if self.params.links && has_safe_link(inlines) {
            let hooks = self.hooks.clone();
            view = view.on_link(move |state: &mut State, url: &str| hooks.link(state, url));
        }
        view
    }

    fn paragraph(self, inlines: &[MdInline]) -> RichTextView {
        self.text(inlines, BODY, self.params.selectable)
    }

    fn heading(self, level: u8, inlines: &[MdInline]) -> RichTextView {
        self.text(inlines, heading_base(level), self.params.selectable)
    }

    fn cell(self, inlines: &[MdInline], header: bool) -> RichTextView {
        let base = Base {
            bold: header,
            ..BODY
        };
        self.text(inlines, base, false)
    }

    fn block(self, block: &MdBlock) -> AnyView<State> {
        match block {
            MdBlock::Paragraph(content) => any(self.paragraph(content)),
            MdBlock::Heading { level, content } => {
                any(semantics(self.heading(*level, content)).heading(usize::from(*level)))
            }
            MdBlock::CodeBlock {
                language,
                code,
                closed,
            } => {
                let hooks = self.hooks.clone();
                let mut view = code_block::<State>(code.as_str())
                    .streaming(self.params.live && !closed)
                    .on_copy(move |state: &mut State, code: &str| hooks.copy(state, code));
                if let Some(language) = language {
                    view = view.language(language.as_str());
                }
                any(view)
            }
            MdBlock::List {
                ordered,
                start,
                items,
            } => any(self.list(*ordered, *start, items)),
            MdBlock::BlockQuote(blocks) => any(GutterView {
                kind: GutterKind::Quote,
                views: vec![any(self.stack(blocks, style::BLOCK_GAP))],
                speaks: Vec::new(),
            }),
            MdBlock::Table {
                alignments,
                header,
                rows,
            } => {
                let header = header.iter().map(|c| self.cell(c, true)).collect();
                let rows = rows
                    .iter()
                    .map(|row| row.iter().map(|c| self.cell(c, false)).collect())
                    .collect();
                any(table::<State>(alignments.clone(), header, rows).streaming(self.params.live))
            }
            MdBlock::Rule => any(RuleView),
        }
    }

    /// `blocks` stacked with `gap` between them.
    fn stack(self, blocks: &[MdBlock], gap: f64) -> FlexView<State> {
        let children: Vec<AnyView<State>> = blocks.iter().map(|b| self.block(b)).collect();
        column::<State>().gap(gap).children(children)
    }

    fn list(self, ordered: bool, start: u64, items: &[MdListItem]) -> GutterView<State> {
        let inner = Render {
            depth: self.depth + 1,
            ..self
        };
        let bullet = if self.depth.is_multiple_of(2) {
            BULLET
        } else {
            "-"
        };
        let mut views = Vec::with_capacity(items.len() * 2);
        let mut speaks = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            // A bullet says nothing; a number or a checkbox does.
            speaks.push(ordered || item.checked.is_some());
            let marker = match item.checked {
                Some(checked) => any(CheckView { checked }),
                None if ordered => {
                    let number = start.saturating_add(index as u64);
                    any(marker_text(format!("{number}.")))
                }
                None => any(marker_text(bullet.to_owned())),
            };
            views.push(marker);
            views.push(any(inner.stack(&item.blocks, style::ROW_GAP)));
        }
        GutterView {
            kind: GutterKind::List { ordered },
            views,
            speaks,
        }
    }
}

fn marker_text(text: String) -> RichTextView {
    rich_text([span(text)
        .size(BODY_SIZE)
        .themed_mono_family()
        .themed_role(ThemeTextColor::OnSurfaceVariant)])
}

/// The retained widget for a [`MarkdownView`].
pub struct MarkdownWidget {
    blocks: Vec<MdBlock>,
    params: Vec<Params>,
    children: Vec<ChildPod>,
    /// The views the children were built from: `Vec<AnyView<State>>`.
    views: Box<dyn Any>,
    /// The shared callbacks: `Rc<Hooks<State>>`.
    hooks: Box<dyn Any>,
    streaming: bool,
    indicator_y: f64,
    /// How many times the source has been parsed.
    #[cfg(test)]
    parses: usize,
    /// How many existing block widgets the last rebuild re-rendered.
    #[cfg(test)]
    rebuilt: usize,
}

impl MarkdownWidget {
    fn parse(&mut self, source: &str) -> Vec<MdBlock> {
        if source.len() > MARKDOWN_MAX_SOURCE_BYTES {
            return plain_blocks(source);
        }
        #[cfg(test)]
        {
            self.parses += 1;
        }
        parse_markdown(source)
    }
}

/// `source` as plain text, unparsed: paragraphs of about
/// [`PLAIN_CHUNK_BYTES`] cut at line ends (a longer line at a character
/// boundary), each line ending in a hard break, so appends stay cheap.
fn plain_blocks(source: &str) -> Vec<MdBlock> {
    let mut blocks = Vec::new();
    let mut lines: Vec<MdInline> = Vec::new();
    let mut bytes = 0;
    let mut flush = |lines: &mut Vec<MdInline>, bytes: &mut usize| {
        if !lines.is_empty() {
            blocks.push(MdBlock::Paragraph(std::mem::take(lines)));
        }
        *bytes = 0;
    };
    for line in source.split('\n') {
        let mut rest = line.strip_suffix('\r').unwrap_or(line);
        loop {
            let mut cut = rest.len().min(PLAIN_CHUNK_BYTES);
            while !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            let (piece, tail) = rest.split_at(cut);
            if !lines.is_empty() {
                lines.push(MdInline::HardBreak);
            }
            lines.push(MdInline::Text(piece.to_owned()));
            bytes += piece.len();
            if bytes >= PLAIN_CHUNK_BYTES {
                flush(&mut lines, &mut bytes);
            }
            if tail.is_empty() {
                break;
            }
            rest = tail;
        }
    }
    flush(&mut lines, &mut bytes);
    blocks
}

impl<State: 'static> View<State> for MarkdownView<State> {
    type Element = MarkdownWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MarkdownWidget {
        let hooks = Rc::new(Hooks {
            on_link: RefCell::new(None),
            on_copy: RefCell::new(None),
        });
        self.install(&hooks);
        let mut element = MarkdownWidget {
            blocks: Vec::new(),
            params: Vec::new(),
            children: Vec::new(),
            views: Box::new(()),
            hooks: Box::new(()),
            streaming: self.streaming,
            indicator_y: 0.0,
            #[cfg(test)]
            parses: 0,
            #[cfg(test)]
            rebuilt: 0,
        };
        element.blocks = element.parse(&self.source);
        element.params = self.params(&element.blocks);
        let views: Vec<AnyView<State>> = element
            .blocks
            .iter()
            .zip(&element.params)
            .map(|(block, params)| Render::new(&hooks, *params).block(block))
            .collect();
        element.children = views.iter().map(|view| build_child(view, ctx)).collect();
        element.views = Box::new(views);
        element.hooks = Box::new(hooks);
        element
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MarkdownWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let Some(hooks) = element.hooks.downcast_ref::<Rc<Hooks<State>>>().cloned() else {
            return ChangeFlags::NONE;
        };
        self.install(&hooks);
        let reparsed = (prev.source != self.source).then(|| element.parse(&self.source));
        let blocks: &[MdBlock] = reparsed.as_deref().unwrap_or(&element.blocks);
        let params = self.params(blocks);
        let Some(views) = element.views.downcast_mut::<Vec<AnyView<State>>>() else {
            return ChangeFlags::NONE;
        };
        let mut flags = ChangeFlags::NONE;
        let mut rebuilt = 0;
        let old_len = element.children.len();
        let kept = old_len.min(blocks.len());
        for (index, pod) in element.children.iter_mut().enumerate().take(kept) {
            let same = element.params[index] == params[index]
                && (reparsed.is_none() || element.blocks[index] == blocks[index]);
            if same {
                // Unchanged: an equal view, so nothing is re-shaped; the pass
                // still reaches a selectable block's toolbar.
                let view = &views[index];
                flags |= rebuild_child(view, view, pod, ctx);
            } else {
                let next = Render::new(&hooks, params[index]).block(&blocks[index]);
                flags |= rebuild_child(&views[index], &next, pod, ctx);
                views[index] = next;
                rebuilt += 1;
            }
        }
        for (view, pod) in views.iter().zip(&mut element.children).skip(kept) {
            teardown_child(view, pod, ctx);
        }
        views.truncate(kept);
        element.children.truncate(kept);
        for (block, params) in blocks.iter().zip(&params).skip(kept) {
            let view = Render::new(&hooks, *params).block(block);
            element.children.push(build_child(&view, ctx));
            views.push(view);
        }
        if old_len != blocks.len() || prev.streaming != self.streaming {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.streaming = self.streaming;
        element.params = params;
        if let Some(blocks) = reparsed {
            element.blocks = blocks;
        }
        #[cfg(test)]
        {
            element.rebuilt = rebuilt;
        }
        #[cfg(not(test))]
        let _ = rebuilt;
        flags
    }

    fn teardown(&self, element: &mut MarkdownWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(views) = element.views.downcast_ref::<Vec<AnyView<State>>>() {
            for (view, pod) in views.iter().zip(&mut element.children) {
                teardown_child(view, pod, ctx);
            }
        }
    }
}

impl Widget for MarkdownWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = style::width_of(bc);
        let child_bc = BoxConstraints::loose(Size::new(width, f64::INFINITY));
        let mut y = 0.0;
        for (index, pod) in self.children.iter_mut().enumerate() {
            if index > 0 {
                y += style::BLOCK_GAP;
            }
            let size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::new(0.0, y));
            y += size.height;
        }
        if self.streaming {
            if !self.children.is_empty() {
                y += style::BLOCK_GAP;
            }
            self.indicator_y = y;
            y += INDICATOR_H;
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
        if self.streaming {
            let color = style::muted(Theme::from_paint_ctx(ctx));
            let o = ctx.origin();
            let top = o.y + self.indicator_y + (INDICATOR_H - DOT) / 2.0;
            for dot in 0..3 {
                let x = o.x + f64::from(dot) * DOT * 2.0;
                scene.fill_rounded_rect(Point::new(x, top), Size::new(DOT, DOT), DOT / 2.0, color);
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.children {
            pod.semantics_child(ctx);
        }
    }

    frust::authoring::visit_children!(children);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GutterKind {
    /// A list; `views` alternate marker and item body.
    List { ordered: bool },
    /// A block quote; `views` is its one body.
    Quote,
}

/// Content set off by a gutter on its left: list markers or a quote bar.
struct GutterView<State: 'static> {
    kind: GutterKind,
    views: Vec<AnyView<State>>,
    /// Per list item: whether its marker is announced.
    speaks: Vec<bool>,
}

/// The retained widget for a [`GutterView`].
struct GutterWidget {
    kind: GutterKind,
    pods: Vec<ChildPod>,
    speaks: Vec<bool>,
}

impl<State: 'static> View<State> for GutterView<State> {
    type Element = GutterWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GutterWidget {
        GutterWidget {
            kind: self.kind,
            pods: self.views.iter().map(|v| build_child(v, ctx)).collect(),
            speaks: self.speaks.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GutterWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.kind != self.kind || prev.views.len() != self.views.len() {
            element.kind = self.kind;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.speaks.clone_from(&self.speaks);
        let kept = prev.views.len().min(self.views.len());
        for (index, pod) in element.pods.iter_mut().enumerate().take(kept) {
            flags |= rebuild_child(&prev.views[index], &self.views[index], pod, ctx);
        }
        for (view, pod) in prev.views.iter().zip(&mut element.pods).skip(kept) {
            teardown_child(view, pod, ctx);
        }
        element.pods.truncate(kept);
        for view in &self.views[kept..] {
            element.pods.push(build_child(view, ctx));
        }
        flags
    }

    fn teardown(&self, element: &mut GutterWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.views.iter().zip(&mut element.pods) {
            teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for GutterWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = style::width_of(bc);
        let GutterKind::List { ordered } = self.kind else {
            let body_bc =
                BoxConstraints::loose(Size::new((width - QUOTE_INDENT).max(0.0), f64::INFINITY));
            let height = self.pods.first_mut().map_or(0.0, |pod| {
                pod.set_origin(Point::new(QUOTE_INDENT, 0.0));
                pod.layout_child(ctx, &body_bc).height
            });
            return bc.constrain(Size::new(width, height));
        };
        let marker_bc = BoxConstraints::loose(Size::new(width, f64::INFINITY));
        let markers: Vec<Size> = self
            .pods
            .iter_mut()
            .step_by(2)
            .map(|pod| pod.layout_child(ctx, &marker_bc))
            .collect();
        let widest = markers.iter().map(|s| s.width).fold(0.0, f64::max);
        let indent = (widest + MARKER_GAP).max(LIST_INDENT).min(width);
        let body_bc = BoxConstraints::loose(Size::new((width - indent).max(0.0), f64::INFINITY));
        let mut y = 0.0;
        for (row, pair) in self.pods.chunks_mut(2).enumerate() {
            if row > 0 {
                y += style::ROW_GAP;
            }
            let marker = markers[row];
            let x = if ordered {
                (indent - MARKER_GAP - marker.width).max(0.0)
            } else {
                0.0
            };
            pair[0].set_origin(Point::new(x, y));
            let body_h = pair.get_mut(1).map_or(0.0, |body| {
                body.set_origin(Point::new(indent, y));
                body.layout_child(ctx, &body_bc).height
            });
            y += marker.height.max(body_h);
        }
        bc.constrain(Size::new(width, y))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if self.kind == GutterKind::Quote {
            let color = style::border(Theme::from_paint_ctx(ctx));
            let size = Size::new(QUOTE_BAR_W, ctx.size().height);
            scene.fill_rounded_rect(ctx.origin(), size, QUOTE_BAR_W / 2.0, color);
        }
        for pod in &mut self.pods {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event(&mut self.pods, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.kind == GutterKind::Quote {
            ctx.push_container(
                Role::Blockquote,
                |_| {},
                |ctx| self.pods.iter().for_each(|pod| pod.semantics_child(ctx)),
            );
            return;
        }
        let items = self.pods.len().div_ceil(2);
        ctx.push_container(
            Role::List,
            |node| node.set_size_of_set(items),
            |ctx| {
                for (index, pair) in self.pods.chunks(2).enumerate() {
                    ctx.push_container(
                        Role::ListItem,
                        |node| node.set_position_in_set(index + 1),
                        |ctx| {
                            if self.speaks.get(index).copied().unwrap_or(false) {
                                pair[0].semantics_child(ctx);
                            }
                            if let Some(body) = pair.get(1) {
                                body.semantics_child(ctx);
                            }
                        },
                    );
                }
            },
        );
    }

    frust::authoring::visit_children!(pods);
}

/// A drawn, non-interactive task-list checkbox.
struct CheckView {
    checked: bool,
}

struct CheckWidget {
    checked: bool,
}

impl<State: 'static> View<State> for CheckView {
    type Element = CheckWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> CheckWidget {
        CheckWidget {
            checked: self.checked,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut CheckWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.checked = self.checked;
        if prev.checked == self.checked {
            ChangeFlags::NONE
        } else {
            ChangeFlags::PAINT
        }
    }
}

impl Widget for CheckWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(CHECK_SIDE, CHECK_SIDE + CHECK_TOP * 2.0))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let o = ctx.origin() + frust::authoring::Vec2::new(0.0, CHECK_TOP);
        let side = Size::new(CHECK_SIDE, CHECK_SIDE);
        let mut outline = BezPath::new();
        let r = Rect::from_origin_size(Point::ORIGIN, side);
        outline.move_to(Point::new(r.x0, r.y0));
        outline.line_to(Point::new(r.x1, r.y0));
        outline.line_to(Point::new(r.x1, r.y1));
        outline.line_to(Point::new(r.x0, r.y1));
        outline.close_path();
        let muted = style::muted(theme);
        scene.stroke_path(o, &outline, style::BORDER_WIDTH, &Brush::Solid(muted));
        if self.checked {
            let ink = style::ink(theme);
            let mut mark = BezPath::new();
            mark.move_to(Point::new(CHECK_SIDE * 0.2, CHECK_SIDE * 0.55));
            mark.line_to(Point::new(CHECK_SIDE * 0.42, CHECK_SIDE * 0.78));
            mark.line_to(Point::new(CHECK_SIDE * 0.82, CHECK_SIDE * 0.25));
            scene.stroke_path(o, &mark, style::BORDER_WIDTH * 1.5, &Brush::Solid(ink));
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::CheckBox, |node| {
            node.set_toggled(if self.checked {
                Toggled::True
            } else {
                Toggled::False
            });
            node.set_read_only();
        });
    }
}

/// A thematic break.
struct RuleView;

struct RuleWidget;

impl<State: 'static> View<State> for RuleView {
    type Element = RuleWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RuleWidget {
        RuleWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut RuleWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::NONE
    }
}

impl Widget for RuleWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(style::width_of(bc), RULE_H))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let color = style::border(Theme::from_paint_ctx(ctx));
        let o = ctx.origin();
        let y = o.y + RULE_H / 2.0;
        let width = ctx.size().width;
        scene.stroke_line(
            Point::new(o.x, y),
            Point::new(o.x + width, y),
            style::BORDER_WIDTH,
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::markdown_parse::stable_prefix_len;
    use frust::FrameTime;
    use frust::accesskit::Toggled;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::{TextContext, TextStyle};
    use frust::authoring::{Color, PointerButton, PointerEvent, PointerPhase, WidgetId};
    use frust_core::{InspectDetail, InspectNode, RenderRoot};

    #[derive(Default)]
    struct App {
        links: Vec<String>,
        copied: Vec<String>,
    }

    type Root = RenderRoot<App, AnyView<App>>;

    struct Rig {
        root: Root,
        app: App,
        text: TextContext,
        window: Size,
    }

    impl Rig {
        fn new(themed: bool, width: f64) -> Self {
            let mut root: Root = RenderRoot::new();
            if themed {
                root.set_theme(Box::new(crate::baseline()));
            }
            Rig {
                root,
                app: App::default(),
                text: TextContext::new(),
                window: Size::new(width, 4000.0),
            }
        }

        fn rebuild(&mut self, mut view: impl FnMut(&mut App) -> MarkdownView<App>) {
            self.root
                .rebuild(&mut |s: &mut App| any(view(s)), &mut self.app);
        }

        fn layout(&mut self) {
            self.root
                .layout_with_text(self.window, &mut self.text as &mut dyn Any);
        }

        fn show(&mut self, view: impl FnMut(&mut App) -> MarkdownView<App>) {
            self.rebuild(view);
            self.layout();
        }

        fn nodes(&self) -> Vec<InspectNode> {
            self.root.inspect()
        }

        fn node(&self, ty: &str) -> Vec<Rect> {
            self.nodes()
                .into_iter()
                .filter(|n| n.type_name.ends_with(ty))
                .map(|n| n.bounds)
                .collect()
        }

        /// The top-level block pods: the markdown root's direct children.
        fn blocks(&self) -> Vec<InspectNode> {
            let nodes = self.nodes();
            let root = nodes[0].id;
            nodes
                .into_iter()
                .filter(|n| n.parent == Some(root))
                .collect()
        }

        /// The laid-out text and bounds of every `rich_text`.
        fn texts(&self) -> Vec<(String, Rect)> {
            self.nodes()
                .into_iter()
                .filter_map(|n| match n.detail {
                    Some(InspectDetail::Text(t)) if n.type_name.ends_with("RichTextWidget") => {
                        Some((t.text, n.bounds))
                    }
                    _ => None,
                })
                .collect()
        }

        fn text_rect(&self, starts: &str) -> Rect {
            self.texts()
                .into_iter()
                .find(|(t, _)| t.starts_with(starts))
                .unwrap_or_else(|| panic!("no text {starts:?} in {:?}", self.texts()))
                .1
        }

        fn click(&mut self, at: Point) {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                let e = InputEvent::Pointer(PointerEvent {
                    phase,
                    position: at,
                    button: PointerButton::Primary,
                });
                self.root.event(&mut self.app, &e);
            }
        }
    }

    const STREAMED: &str = "# Plan\n\
        \n\
        Some *emphasis*, **strong**, ~~gone~~, `code` and a [link](https://example.com/a).\n\
        \n\
        - first item\n\
        - [x] done task\n\
        - [ ] open task\n\
        \x20 - nested\n\
        \n\
        1. one\n\
        2. two\n\
        \n\
        > quoted *text*\n\
        \n\
        | name | kind |\n\
        | :--- | ---: |\n\
        | alpha | beta |\n\
        | gamma | delta |\n\
        \n\
        ```rust\n\
        fn main() {}\n\
        ```\n\
        \n\
        ---\n\
        \n\
        Final words of the message.\n";

    /// `source` cut into `n` growing prefixes at character boundaries.
    fn prefixes(source: &str, n: usize) -> Vec<&str> {
        (1..=n)
            .map(|k| {
                let mut at = source.len() * k / n;
                while !source.is_char_boundary(at) {
                    at += 1;
                }
                &source[..at]
            })
            .collect()
    }

    fn hooks() -> Rc<Hooks<App>> {
        Rc::new(Hooks {
            on_link: RefCell::new(None),
            on_copy: RefCell::new(None),
        })
    }

    fn params() -> Params {
        Params {
            selectable: true,
            links: true,
            live: false,
        }
    }

    #[test]
    fn heading_and_body_sizes_follow_the_glyph_type_scale() {
        let scale = crate::tokens::scales::type_scale(&TextStyle::default());
        assert_eq!(HEADING_SIZES[0], scale.headline_large.size);
        assert_eq!(HEADING_SIZES[1], scale.title_large.size);
        assert_eq!(HEADING_SIZES[2], scale.body_large.size);
        assert_eq!(BODY_SIZE, scale.body_large.size);
    }

    #[test]
    fn a_source_past_the_limit_is_plain_text_and_returns_promptly() {
        let line = "# not a heading **not bold** [not a link](javascript:x)\n";
        let mut source = line.repeat((1 << 20) / line.len());
        source.push_str(&"x".repeat(64 * 1024));
        assert!(source.len() >= 1 << 20);
        let started = std::time::Instant::now();
        let mut counter = 0u64;
        let view = markdown::<App>(source.as_str());
        let widget = View::<App>::build(&view, &mut BuildCtx::new(&mut counter));
        assert_eq!(widget.parses, 0, "past the limit nothing is parsed");
        assert!(
            widget
                .blocks
                .iter()
                .all(|b| matches!(b, MdBlock::Paragraph(_))),
            "plain paragraphs only"
        );
        let mut rig = Rig::new(true, 320.0);
        rig.show(|_| markdown(source.as_str()));
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "took {elapsed:?}"
        );
        let texts = rig.texts();
        assert!(texts[0].0.starts_with("# not a heading **not bold**"));
        for (_, rect) in &texts {
            assert!(rect.x1 <= 320.0 + 0.01, "{rect:?} past the width");
        }
        let shown: usize = texts.iter().map(|(t, _)| t.len()).sum();
        assert!(
            shown + texts.len() * 64 >= source.len(),
            "every byte is shown"
        );
        let small = markdown::<App>("# heading");
        let small = View::<App>::build(&small, &mut BuildCtx::new(&mut counter));
        assert_eq!(small.parses, 1, "under the limit it parses");
    }

    #[test]
    fn an_equal_source_is_not_parsed_again() {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let a = markdown::<App>("one\n\ntwo");
        let mut widget = View::<App>::build(&a, &mut ctx);
        assert_eq!(widget.parses, 1);
        let b = markdown::<App>("one\n\ntwo");
        let flags = View::<App>::rebuild(&b, &a, &mut widget, &mut ctx);
        assert_eq!((widget.parses, widget.rebuilt), (1, 0));
        assert_eq!(flags, ChangeFlags::NONE);
        let c = markdown::<App>("one\n\ntwo!");
        View::<App>::rebuild(&c, &b, &mut widget, &mut ctx);
        assert_eq!((widget.parses, widget.rebuilt), (2, 1));
        let d = markdown::<App>("one\n\ntwo!").selectable(false);
        View::<App>::rebuild(&d, &c, &mut widget, &mut ctx);
        assert_eq!(
            widget.parses, 2,
            "a builder change re-renders, never re-parses"
        );
        assert_eq!(widget.rebuilt, 2);
    }

    #[test]
    fn each_appended_chunk_rebuilds_at_most_two_block_widgets() {
        let chunks = prefixes(STREAMED, 40);
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        let mut prev = markdown::<App>(chunks[0]).streaming(true);
        let mut widget = View::<App>::build(&prev, &mut ctx);
        let hooks = hooks();
        for (step, prefix) in chunks.iter().enumerate().skip(1) {
            let next = markdown::<App>(*prefix).streaming(step + 1 < chunks.len());
            View::<App>::rebuild(&next, &prev, &mut widget, &mut ctx);
            assert!(
                widget.rebuilt <= 2,
                "{prefix:?}: {} rebuilt",
                widget.rebuilt
            );
            assert_eq!(widget.parses, step + 1);
            let (before, after) = (parse_markdown(prev.source.as_str()), parse_markdown(prefix));
            for (old, new) in before
                .iter()
                .zip(&after)
                .take(stable_prefix_len(&before, &after))
            {
                if let (MdBlock::Paragraph(a), MdBlock::Paragraph(b)) = (old, new) {
                    let render = Render::new(&hooks, params());
                    assert_eq!(render.paragraph(a), render.paragraph(b));
                }
            }
            prev = next;
        }
        assert_eq!(widget.children.len(), parse_markdown(STREAMED).len());
    }

    fn dirty_blocks(rig: &Rig) -> Vec<bool> {
        let dirty: std::collections::HashMap<WidgetId, bool> =
            rig.root.inspect_layout_dirty().into_iter().collect();
        rig.blocks().iter().map(|n| dirty[&n.id]).collect()
    }

    #[test]
    fn streaming_in_forty_chunks_leaves_the_settled_blocks_clean() {
        let mut rig = Rig::new(true, 360.0);
        let chunks = prefixes(STREAMED, 40);
        let mut prev: &str = "";
        for prefix in &chunks {
            rig.rebuild(|_| markdown(*prefix).streaming(true));
            let (before, after) = (parse_markdown(prev), parse_markdown(prefix));
            // The previous tail may have been the live table: it settles.
            let stable = stable_prefix_len(&before, &after).min(before.len().saturating_sub(1));
            let dirty = dirty_blocks(&rig);
            assert_eq!(dirty.len(), after.len());
            assert!(
                dirty[..stable].iter().all(|d| !d),
                "{prefix:?}: settled blocks dirty {dirty:?}"
            );
            let touched = dirty[..before.len().min(after.len())]
                .iter()
                .filter(|d| **d)
                .count();
            assert!(touched <= 2, "{prefix:?}: {touched} earlier blocks dirty");
            rig.layout();
            prev = prefix;
        }
        // Re-showing the finished text shapes nothing new.
        let shapes = rig.text.shape_cache_stats().shapes;
        rig.show(|_| markdown(STREAMED).streaming(true));
        assert_eq!(rig.text.shape_cache_stats().shapes, shapes);
    }

    fn all_kinds() -> String {
        let token = "x".repeat(500);
        let wide_header: String = (0..8).map(|c| format!("| column {c} ")).collect();
        let wide_rule = "| --- ".repeat(8);
        let wide_row: String = (0..8)
            .map(|c| format!("| a fairly long cell {c} "))
            .collect();
        format!(
            "# Heading one\n\n### Heading three\n\n{token}\n\n\
             A paragraph with *em*, **strong**, ~~strike~~, `code` and <b>html</b>.\n\n\
             - item\n  - nested {token}\n- [x] done\n\n3. three\n4. four\n\n\
             > quote {token}\n\n```\nlet long = \"{token}\";\n```\n\n---\n\n\
             {wide_header}|\n{wide_rule}|\n{wide_row}|\n\n<div>\nraw\n</div>\n"
        )
    }

    #[test]
    fn every_block_kind_lays_out_within_the_width() {
        let width = 300.0;
        let source = all_kinds();
        for themed in [true, false] {
            let mut rig = Rig::new(themed, width);
            rig.show(|_| markdown(source.as_str()));
            let blocks = rig.blocks();
            assert_eq!(blocks.len(), parse_markdown(&source).len());
            for block in &blocks {
                assert!(
                    block.bounds.x1 <= width + 0.01 && block.bounds.height() > 0.0,
                    "{} at {:?}",
                    block.type_name,
                    block.bounds
                );
            }
            let token = rig.text_rect("xxxx");
            assert!(token.width() <= width + 0.01, "token {}", token.width());
            assert!(token.height() > 60.0, "the token wraps: {}", token.height());
            let grid = rig.node("GridWidget")[0];
            assert!(grid.width() > width, "the table is wider: {}", grid.width());
            let table = rig.node("TableWidget")[0];
            assert!(table.x1 <= width + 0.01);
            for (text, rect) in rig.texts() {
                let scrolls = ["a fairly long", "column", "let long"];
                if scrolls.iter().any(|s| text.starts_with(s)) {
                    continue; // table cells and code lines scroll sideways
                }
                assert!(rect.x1 <= width + 0.01, "{text:?} at {rect:?}");
            }
        }
    }

    #[test]
    fn a_wide_table_scrolls_sideways_inside_its_block() {
        let source = all_kinds();
        let mut rig = Rig::new(true, 300.0);
        rig.show(|_| markdown(source.as_str()));
        let viewport = rig
            .node("ViewportWidget")
            .into_iter()
            .find(|r| (r.width() - 300.0).abs() < 0.01 && r.height() > 30.0)
            .unwrap_or_else(|| panic!("{:?}", rig.node("ViewportWidget")));
        let table = rig.node("TableWidget")[0];
        assert!((viewport.height() - table.height()).abs() < 0.01);
        assert!(viewport.x1 <= 300.01);
    }

    #[test]
    fn table_columns_honour_their_alignment() {
        let source = "| left | mid | right |\n| :--- | :-: | ---: |\n\
                      | a | b | c |\n| longer cell | longer cell | longer cell |\n";
        let mut rig = Rig::new(true, 600.0);
        rig.show(|_| markdown(source));
        let texts = rig.texts();
        let find = |name: &str| texts.iter().find(|(t, _)| t == name).map(|(_, r)| *r);
        let long: Vec<Rect> = texts
            .iter()
            .filter(|(t, _)| t == "longer cell")
            .map(|(_, r)| *r)
            .collect();
        let (a, b, c) = (find("a").unwrap(), find("b").unwrap(), find("c").unwrap());
        assert!((a.x0 - long[0].x0).abs() < 0.5, "left");
        assert!((b.center().x - long[1].center().x).abs() < 0.5, "center");
        assert!((c.x1 - long[2].x1).abs() < 0.5, "right");
        assert!(
            (find("left").unwrap().y0 - a.y0).abs() > 5.0,
            "header row above"
        );
    }

    #[test]
    fn an_unsafe_link_is_plain_text_with_its_url_and_never_fires() {
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| {
            markdown("[click](javascript:alert(1)) after")
                .on_link(|app: &mut App, url| app.links.push(url.to_owned()))
        });
        assert_eq!(rig.texts()[0].0, "click (javascript:alert(1)) after");
        let rect = rig.text_rect("click");
        rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
        assert!(rig.app.links.is_empty());
        let update = rig.root.semantics();
        assert!(!update.nodes.iter().any(|(_, n)| n.role() == Role::Link));
    }

    #[test]
    fn a_safe_link_fires_on_link_with_the_exact_url() {
        let url = "https://example.com/a?b=1&c=%20";
        let source = format!("[docs]({url}) and the rest");
        for selectable in [true, false] {
            let mut rig = Rig::new(true, 400.0);
            rig.show(|_| {
                markdown(source.as_str())
                    .selectable(selectable)
                    .on_link(|app: &mut App, url| app.links.push(url.to_owned()))
            });
            let rect = rig.text_rect("docs");
            rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
            assert_eq!(rig.app.links, [url.to_owned()], "selectable {selectable}");
            let update = rig.root.semantics();
            assert!(
                update
                    .nodes
                    .iter()
                    .any(|(_, n)| n.role() == Role::Link && n.label() == Some("docs"))
            );
        }
    }

    #[test]
    fn on_link_receives_exactly_the_url_that_was_validated() {
        // (source, label, what on_link receives)
        let cases = [
            (
                "[padded](<  https://example.com/a  >) tail",
                "padded",
                Some("https://example.com/a"),
            ),
            ("[ctl](<\u{1}https://example.com/b>) tail", "ctl", None),
        ];
        for (source, label, received) in cases {
            let mut rig = Rig::new(true, 400.0);
            rig.show(|_| {
                markdown(source).on_link(|app: &mut App, url| app.links.push(url.to_owned()))
            });
            let rect = rig.text_rect(label);
            rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
            let expected: Vec<String> = received.into_iter().map(str::to_owned).collect();
            assert_eq!(rig.app.links, expected, "{source:?}");
            if received.is_none() {
                let shown = &rig.texts()[0].0;
                assert_eq!(shown, "ctl (?https://example.com/b) tail");
            }
        }
    }

    #[test]
    fn link_labels_and_shown_urls_show_hidden_characters() {
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| {
            markdown("[a\u{202E}b\u{200B}c\u{2066}d\u{FEFF}e](https://x.example/) and [f](<javascript:\u{202D}g\u{200F}>)")
                .on_link(|app: &mut App, url| app.links.push(url.to_owned()))
        });
        assert_eq!(rig.texts()[0].0, "a?b?c?d?e and f (javascript:?g?)");
        assert_eq!(shown_url("x\u{2069}\n\u{200C}y"), "x??\u{200C}y");
        let rect = rig.text_rect("a?b");
        rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
        assert_eq!(
            rig.app.links,
            ["https://x.example/".to_owned()],
            "still tappable"
        );
    }

    #[test]
    fn a_label_naming_another_host_is_not_tappable() {
        let source = "[https://bank.example/login](https://evil.example/) tail";
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| markdown(source).on_link(|app: &mut App, url| app.links.push(url.to_owned())));
        assert_eq!(
            rig.texts()[0].0,
            "https://bank.example/login (https://evil.example/) tail"
        );
        let rect = rig.text_rect("https://bank");
        rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
        assert!(rig.app.links.is_empty());
        let update = rig.root.semantics();
        assert!(!update.nodes.iter().any(|(_, n)| n.role() == Role::Link));

        let same = "[HTTPS://Docs.Example:443/](https://user@docs.example/a) tail";
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| markdown(same).on_link(|app: &mut App, url| app.links.push(url.to_owned())));
        let rect = rig.text_rect("HTTPS://Docs");
        rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
        assert_eq!(rig.app.links, ["https://user@docs.example/a".to_owned()]);
        assert_eq!(http_host("http://[::1]:80/x").as_deref(), Some("::1"));
        assert_eq!(http_host("docs.example"), None, "a bare name is not a URL");
        assert_eq!(http_host("mailto:a@b"), None);
    }

    #[test]
    fn a_link_keeps_reaching_the_latest_callback_across_rebuilds() {
        let mut rig = Rig::new(true, 400.0);
        let source = "[docs](https://x.example/) tail";
        rig.show(|_| markdown(source).on_link(|app: &mut App, _| app.links.push("old".into())));
        rig.show(|_| markdown(source).on_link(|app: &mut App, _| app.links.push("new".into())));
        let rect = rig.text_rect("docs");
        rig.click(Point::new(rect.x0 + 6.0, rect.y0 + 6.0));
        assert_eq!(rig.app.links, ["new".to_owned()]);
    }

    #[test]
    fn a_code_block_copy_reaches_on_copy() {
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| {
            markdown("intro\n\n```sh\necho hi\n```\n")
                .on_copy(|app: &mut App, code| app.copied.push(code.to_owned()))
        });
        let block = rig.node("CodeBlockWidget")[0];
        rig.click(Point::new(block.x1 - style::PAD, block.y0 + 8.0));
        assert_eq!(rig.app.copied, ["echo hi".to_owned()]);
    }

    #[test]
    fn the_streaming_indicator_adds_a_row_without_moving_the_blocks() {
        let source = "para one\n\n- item\n\n```\nopen fence";
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| markdown(source).streaming(true));
        let live = rig.nodes()[0].bounds;
        let live_blocks: Vec<Rect> = rig.blocks().iter().map(|n| n.bounds).collect();
        rig.show(|_| markdown(source).streaming(false));
        let done = rig.nodes()[0].bounds;
        let done_blocks: Vec<Rect> = rig.blocks().iter().map(|n| n.bounds).collect();
        assert!(live.height() > done.height() + INDICATOR_H - 0.5);
        assert_eq!(live_blocks, done_blocks);
    }

    #[test]
    fn only_an_unclosed_last_fence_or_table_is_live() {
        let view = markdown::<App>("x").streaming(true);
        let blocks = parse_markdown("```\nclosed\n```\n\n| a |\n| - |\n| b |\n\n```\nopen");
        let live: Vec<bool> = view.params(&blocks).iter().map(|p| p.live).collect();
        assert_eq!(live, [false, false, true]);
        let blocks = parse_markdown("| a |\n| - |\n| b |");
        assert!(view.params(&blocks)[0].live);
        assert!(!markdown::<App>("x").params(&blocks)[0].live);
    }

    #[test]
    fn semantics_expose_headings_lists_tables_and_task_state() {
        let source =
            "# Title\n\n## Sub\n\n- a\n- [x] b\n\n| h1 | h2 |\n| - | - |\n| c | d |\n\n> q\n";
        let mut rig = Rig::new(true, 400.0);
        rig.show(|_| markdown(source));
        let update = rig.root.semantics();
        let count = |role: Role| {
            update
                .nodes
                .iter()
                .filter(|(_, n)| n.role() == role)
                .count()
        };
        // The platform level is zero-based: `#` is 0, `##` is 1.
        let level = |l: usize| {
            update
                .nodes
                .iter()
                .any(|(_, n)| n.role() == Role::Heading && n.level() == Some(l))
        };
        assert!(level(0) && level(1));
        assert_eq!(count(Role::List), 1);
        assert_eq!(count(Role::ListItem), 2);
        assert!(update.nodes.iter().any(|(_, n)| {
            n.role() == Role::CheckBox && n.toggled() == Some(Toggled::True) && n.is_read_only()
        }));
        assert_eq!(count(Role::Table), 1);
        assert_eq!(count(Role::ColumnHeader), 2);
        assert_eq!(count(Role::Cell), 2);
        assert_eq!(count(Role::Blockquote), 1);
    }

    #[derive(Default)]
    struct Count(usize);

    impl PaintScene for Count {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.0 += 1;
        }
    }

    #[test]
    fn themed_and_unthemed_paint_draw_without_panicking() {
        let source = all_kinds();
        for themed in [true, false] {
            let mut rig = Rig::new(themed, 320.0);
            rig.show(|_| markdown(source.as_str()).streaming(true));
            let mut scene = Count::default();
            rig.root.paint(&mut scene, FrameTime::ZERO);
            assert!(scene.0 > 10, "themed {themed}: {} runs", scene.0);
        }
    }

    #[test]
    fn an_empty_source_has_no_blocks() {
        let mut rig = Rig::new(true, 320.0);
        rig.show(|_| markdown(""));
        assert!(rig.blocks().is_empty());
        assert_eq!(rig.nodes()[0].bounds.height(), 0.0);
    }
}
