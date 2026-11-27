//! The `rich_text` widget: one paragraph of mixed-style text.
//!
//! [`span`] describes a styled piece of text and [`rich_text`] joins pieces
//! into a [`RichTextView`], which materialises into a retained
//! [`RichTextWidget`]. The widget resolves every span's effective style at
//! layout exactly as [`crate::TextView`] does for a single run (themed colour
//! unless explicit, themed family roles, the theme's text scale for spans
//! without an explicit size), shapes the spans as one paragraph, and paints
//! span backgrounds, underline rects, glyph runs, then line-through rects.
//!
//! [`SpanView::background`] fills behind each line fragment of a span, padded
//! sideways without touching layout. [`RichTextView::on_link`] receives a
//! [`SpanView::link`] id on a primary tap that starts and ends on that link
//! within [`TOUCH_SLOP`]; a press elsewhere is not consumed, and a scroll
//! taking it over cancels it. Each link is a `Role::Link` child of the
//! paragraph node, with no focus stop of its own.
//!
//! Fragments come from the laid-out paragraph's own geometry
//! ([`TextLayout::range_rects`] over each marked group's byte range, newlines
//! left out), set on the line boxes of [`TextLayout::line_bounds`]: no second
//! layout is made for them.
//!
//! # Selection
//!
//! [`RichTextView::selectable`] lets the user select and copy the block's
//! text — one block at a time, never across two. Every rectangle, hit test
//! and word lookup is the layout's own geometry
//! ([`TextLayout::range_rects`], [`TextLayout::position_at_point`],
//! [`TextLayout::word_range_at`]).
//!
//! - **Touch** (a press [`EventCtx::pointer_id`] reports as
//!   [`PointerSource::Touch`]): a stationary long-press selects the word under
//!   the finger, shows a drag handle at each end and raises the selection
//!   toolbar; dragging a handle moves its end by grapheme, and a finger still
//!   down after the long-press extends by grapheme past the word. A tap inside
//!   the selection toggles the toolbar; a tap outside clears the selection. A
//!   drag that is neither scrolls the parent: no scroll veto is raised for it.
//! - **Mouse**: a press-drag selects by grapheme (the press vetoes a parent
//!   scroll's takeover, the desktop convention), a double-click selects a word
//!   (a drag on from it keeps the word and extends by grapheme), a
//!   triple-click selects the whole block, and a secondary press toggles the
//!   toolbar. A held mouse press is not a long-press.
//! - **Keyboard**, while the block holds the selection: the copy chords and
//!   keys `TextInput` decodes copy ([`crate::text_selection::clipboard_verb`]),
//!   the select-all chord selects the block, and Escape clears it; a bare
//!   Escape is then declined so a modal around the block still closes. Cut
//!   and paste are answered with nothing.
//! - **Copy** writes the selected text into the pass's clipboard slot
//!   ([`EventCtx::write_clipboard`]), the route `TextInput` answers a copy on.
//!   An [`EditCommand`] the shell or the toolbar delivers takes the same path.
//! - **The toolbar** is the process-wide [`selection_toolbar_builder`]'s view,
//!   floated through the block's own overlay slot under
//!   [`SelectionToolbarPolicy::Framework`], and the block publishes its
//!   [`SelectionToolbarRequest`] every paint while it holds the selection
//!   under either policy, as `TextInput` does. It offers Copy and Select all
//!   only.
//! - **Links** still fire on a tap; a long-press on a link selects instead.
//!
//! The block takes focus when its selection starts and drops the selection
//! when it loses focus, so a selection begun in another block, or a text
//! field taking focus, clears this one. A press on a selectable block keeps
//! another widget's focus session ([`EventCtx::keep_focus`]): only a gesture
//! that selects moves focus here, through a bound [`FocusHandle`], so a tap or
//! a scroll on the block leaves a text field's keyboard up. With no session
//! live the press claims focus directly, and a press that ends selecting
//! nothing (a tap, or one a parent scroll took over) releases it again.
//!
//! A double or triple click is measured on the paint clock, so a block that
//! completed a click asks for plain continuation frames until
//! [`DOUBLE_TAP_MS`](crate::gesture) has passed and none after. Replacing
//! the text ends the live press, clears the selection and gives back any focus
//! it no longer needs.
//!
//! The selection survives a scroll and a relayout, and a view change that
//! keeps the text. A disposed block (a virtualized row leaving the window)
//! drops its selection, toolbar and scroll vetoes with it.
//!
//! Assistive technology sees the clipboard verbs on the paragraph node as
//! accesskit custom actions, under `TextInput`'s ids and only while enabled:
//! Copy while a selection exists, Select all while the block is not wholly
//! selected; the node is the focused one while the block holds focus. The
//! selected range itself is not published: an accesskit text selection names
//! positions in `Role::TextRun` nodes, which this paragraph does not have.
//!
//! `selectable(false)`, the default, builds none of this: no gesture state,
//! no focus stop, no handle binding, no overlay slot and no extra semantics.

use std::any::TypeId;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use frust_core::accesskit::{Action, CustomAction, Rect as AccessRect, Role};
use frust_core::event::PointerSource;
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, CursorIcon, EditCommand, EventCtx, EventResult,
    FocusHandle, FrameTime, InputEvent, InspectDetail, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, PointerPhase, SelectionToolbarActions, SelectionToolbarPolicy,
    SelectionToolbarRequest, SemanticsCtx, TOUCH_SLOP, TextInspect, View, Widget,
    selection_toolbar_builder, selection_toolbar_policy,
};
use frust_text::{
    FontFamily, FontStyle, FontWeight, LocaleStamp, OverflowWrap, TextAffinity, TextAlign,
    TextContext, TextDecoration, TextLayout, TextOverflow, TextPosition, TextSpan, TextStyle,
};
use frust_theme::{ColorScheme, ShapeScale, SurfaceRole, Theme};
use kurbo::{Point, Rect, Size, Vec2};
use peniko::Color;

use crate::authoring::{ErasedArgCallback, TypedArgCallback, erase_callback_arg, presses};
use crate::gesture::DOUBLE_TAP_MS;
use crate::overlay::{OverlayAnchor, OverlaySlot};
use crate::text::{
    FamilySource, ThemeTextColor, ThemeTextType, paint_layout, resolve_style, text_inspect,
};
use crate::text_selection::{
    A11Y_COPY_ID, A11Y_SELECT_ALL_ID, ClipboardVerb, HoldTimer, clipboard_verb, handle_color,
    handle_hit, new_toolbar_slot, paint_handle, selection_color,
};

/// How far a span background reaches past its glyphs on each side, in
/// logical px. Paint-only, so layout and line height are unchanged;
/// `frust-theme` has no spacing token to resolve it from.
const BG_PAD_X: f64 = 2.0;
/// Corner radius of a span background without a theme; a theme resolves it
/// from `shape.extra_small`.
const BG_RADIUS: f64 = 3.0;
/// Fill of a [`SpanView::themed_background`] span when no theme is threaded.
const BG_FALLBACK: Color = Color::from_rgb8(0xE5, 0xE7, 0xEB);

/// A styled piece of text inside a [`RichTextView`].
///
/// Carries the per-span subset of the [`crate::TextView`] builders. Colour,
/// family and size resolve per span with the same precedence as a plain text:
/// an explicit choice wins over the theme, and a span with no explicit size
/// follows the theme's text scale.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanView {
    content: String,
    style: TextStyle,
    color_explicit: bool,
    role: ThemeTextColor,
    family_explicit: bool,
    size_explicit: bool,
    family_source: Option<FamilySource>,
    /// The explicit background fill; wins over `background_role`.
    background: Option<Color>,
    background_role: Option<SurfaceRole>,
    link: Option<String>,
}

/// Create a span rendering `content` with default styling.
pub fn span(content: impl Into<String>) -> SpanView {
    SpanView {
        content: content.into(),
        style: TextStyle::default(),
        color_explicit: false,
        role: ThemeTextColor::OnSurface,
        family_explicit: false,
        size_explicit: false,
        family_source: None,
        background: None,
        background_role: None,
        link: None,
    }
}

impl SpanView {
    /// Set the font size, in logical pixels. Marks the size as explicit, so
    /// the theme's text scale no longer applies to this span.
    pub fn size(mut self, size: f32) -> Self {
        self.style.size = size;
        self.size_explicit = true;
        self
    }

    /// Set the glyph fill color. Marks the color as explicit, so it wins over
    /// any themed default.
    pub fn color(mut self, color: Color) -> Self {
        self.style.color = color;
        self.color_explicit = true;
        self
    }

    /// Set the font weight.
    pub fn weight(mut self, weight: FontWeight) -> Self {
        self.style.weight = weight;
        self
    }

    /// Set the font family (or fallback stack). Marks the family as explicit,
    /// so it wins over any [`Self::themed_family`] role.
    pub fn family(mut self, family: FontFamily) -> Self {
        self.style.family = family;
        self.family_explicit = true;
        self
    }

    /// Set the font style to italic.
    pub fn italic(mut self) -> Self {
        self.style.style = FontStyle::Italic;
        self
    }

    /// Set the extra spacing between letters, in logical pixels.
    pub fn letter_spacing(mut self, letter_spacing: f32) -> Self {
        self.style.letter_spacing = letter_spacing;
        self
    }

    /// Tag the span with a BCP 47 locale (`"ja"`, `"th"`, `"zh-Hant"`). Touches
    /// only the locale; color, family and size keep resolving as before.
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.style.locale = Some(locale.into());
        self
    }

    /// Set one OpenType feature, appended after any already set; a later
    /// setting of the same tag wins.
    pub fn font_feature(mut self, tag: [u8; 4], value: u16) -> Self {
        self.style.features.push((tag, value));
        self
    }

    /// Replace the OpenType feature settings with `features`.
    pub fn font_features(mut self, features: impl IntoIterator<Item = ([u8; 4], u16)>) -> Self {
        self.style.features = features.into_iter().collect();
        self
    }

    /// Use tabular (fixed-width) figures, the `tnum` feature.
    pub fn tabular_figures(self) -> Self {
        self.font_feature(*b"tnum", 1)
    }

    /// Underline the span, keeping any other decoration already set.
    pub fn underline(mut self) -> Self {
        self.style.decoration.underline = true;
        self
    }

    /// Strike a line through the span, keeping any other decoration already
    /// set.
    pub fn line_through(mut self) -> Self {
        self.style.decoration.line_through = true;
        self
    }

    /// Replace the span's decoration wholesale.
    pub fn decoration(mut self, decoration: TextDecoration) -> Self {
        self.style.decoration = decoration;
        self
    }

    /// Resolve the span's family from the active theme's type-scale slot at
    /// layout, as [`crate::TextView::themed_family`] does.
    pub fn themed_family(mut self, role: ThemeTextType) -> Self {
        self.family_source = Some(FamilySource::Role(role));
        self
    }

    /// Resolve the span's family from the active theme's monospace family at
    /// layout, as [`crate::TextView::themed_mono_family`] does.
    pub fn themed_mono_family(mut self) -> Self {
        self.family_source = Some(FamilySource::Monospace);
        self
    }

    /// Set the themed default color role used when no explicit color was set.
    pub fn themed_role(mut self, role: ThemeTextColor) -> Self {
        self.role = role;
        self
    }

    /// Fill a small rounded rectangle of `color` behind the span, one per
    /// line fragment it occupies. Marks the background explicit, so it wins
    /// over [`Self::themed_background`].
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Fill the span's background from the active theme's surface `role`,
    /// resolved at paint (a neutral grey with no theme). An explicit
    /// [`Self::background`] wins.
    pub fn themed_background(mut self, role: SurfaceRole) -> Self {
        self.background_role = Some(role);
        self
    }

    /// Mark the span as a link carrying the opaque `id` that
    /// [`RichTextView::on_link`] receives. Adds no styling: colour and
    /// underline stay the caller's. Adjacent spans with the same id form one
    /// link.
    pub fn link(mut self, id: impl Into<String>) -> Self {
        self.link = Some(id.into());
        self
    }

    /// Whether the span needs fragment geometry (a background or a link).
    fn is_marked(&self) -> bool {
        self.background.is_some() || self.background_role.is_some() || self.link.is_some()
    }

    /// Whether `other` paints the same background and belongs to the same
    /// link, so the two may share a shaping group.
    fn same_marks(&self, other: &SpanView) -> bool {
        self.background == other.background
            && self.background_role == other.background_role
            && self.link == other.link
    }

    /// The background fill: explicit, else the themed role, else none.
    fn background_color(&self, theme: Option<&Theme>) -> Option<Color> {
        if let Some(color) = self.background {
            return Some(color);
        }
        let role = self.background_role?;
        Some(theme.map_or(BG_FALLBACK, |t| surface_color(t.scheme(), role)))
    }

    fn effective_style(&self, theme: Option<&Theme>) -> TextStyle {
        resolve_style(
            &self.style,
            self.color_explicit,
            self.role,
            self.family_explicit,
            self.family_source,
            self.size_explicit,
            theme,
        )
    }
}

/// The colour a surface `role` names in `scheme`.
fn surface_color(scheme: &ColorScheme, role: SurfaceRole) -> Color {
    match role {
        SurfaceRole::Surface => scheme.surface,
        SurfaceRole::SurfaceContainerLowest => scheme.surface_container_lowest,
        SurfaceRole::SurfaceContainerLow => scheme.surface_container_low,
        SurfaceRole::SurfaceContainer => scheme.surface_container,
        SurfaceRole::SurfaceContainerHigh => scheme.surface_container_high,
        SurfaceRole::SurfaceContainerHighest => scheme.surface_container_highest,
    }
}

/// Create a rich text from `spans`, laid out as one paragraph.
pub fn rich_text(spans: impl IntoIterator<Item = SpanView>) -> RichTextView {
    RichTextView {
        spans: spans.into_iter().collect(),
        align: None,
        overflow_wrap: None,
        max_lines: None,
        overflow: TextOverflow::default(),
        on_link: LinkHandler::default(),
        selectable: false,
    }
}

/// A declarative description of a paragraph of styled [`SpanView`]s.
///
/// The paragraph knobs ([`Self::align`], [`Self::overflow_wrap`],
/// [`Self::max_lines`], [`Self::overflow`]) apply to the whole paragraph. A
/// truncation ellipsis takes the style of the span it cuts.
#[derive(Clone, Debug, PartialEq)]
pub struct RichTextView {
    spans: Vec<SpanView>,
    align: Option<TextAlign>,
    overflow_wrap: Option<OverflowWrap>,
    max_lines: Option<usize>,
    overflow: TextOverflow,
    on_link: LinkHandler,
    selectable: bool,
}

/// The view-held [`RichTextView::on_link`] callback, erased at the call
/// because the view is not generic over the app state. Equality sees only
/// whether one is set, so a fresh closure each build keeps `prev == self`.
#[derive(Clone, Default)]
struct LinkHandler(Option<Rc<LinkCallback>>);

struct LinkCallback {
    /// The app-state type the callback was written against.
    state: TypeId,
    /// Makes the widget-held adapter; reinstalled on every build and rebuild.
    erase: Box<dyn Fn() -> ErasedArgCallback<String>>,
}

impl PartialEq for LinkHandler {
    fn eq(&self, other: &Self) -> bool {
        self.0.is_some() == other.0.is_some()
    }
}

impl std::fmt::Debug for LinkHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Some(..)" } else { "None" })
    }
}

impl LinkHandler {
    /// The adapter for a view built under `State`, or `None` when no callback
    /// is set or it names another state type (a wiring bug: debug builds
    /// panic, release builds leave the links inert).
    fn install<State: 'static>(&self) -> Option<ErasedArgCallback<String>> {
        let callback = self.0.as_ref()?;
        debug_assert_eq!(
            callback.state,
            TypeId::of::<State>(),
            "RichTextView::on_link names a different state type than the view is built under"
        );
        (callback.state == TypeId::of::<State>()).then(|| (callback.erase)())
    }
}

impl RichTextView {
    /// Set the paragraph alignment; visible once the text wraps to more than
    /// one line.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = Some(align);
        self
    }

    /// Set what a word wider than the line does (CSS `overflow-wrap`).
    pub fn overflow_wrap(mut self, overflow_wrap: OverflowWrap) -> Self {
        self.overflow_wrap = Some(overflow_wrap);
        self
    }

    /// Cap the rendered line count. Content past the limit is handled per
    /// [`Self::overflow`]. `0` renders nothing.
    pub fn max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = Some(max_lines);
        self
    }

    /// Set how content past [`Self::max_lines`] is handled.
    pub fn overflow(mut self, overflow: TextOverflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Run `on_link` against the app state with the id of a
    /// [`SpanView::link`] the user activates: a tap that starts and ends on
    /// that link, or the link node's accessibility Click. `State` is the
    /// state this view is built under; type it in the closure
    /// (`|app: &mut App, id| ..`).
    pub fn on_link<State: 'static>(mut self, on_link: impl Fn(&mut State, &str) + 'static) -> Self {
        let typed: TypedArgCallback<State, String> =
            Rc::new(move |state: &mut State, id: String| on_link(state, &id));
        self.on_link = LinkHandler(Some(Rc::new(LinkCallback {
            state: TypeId::of::<State>(),
            erase: Box::new(move || erase_callback_arg(&typed)),
        })));
        self
    }

    /// Let the user select and copy this block's text: a long-press with
    /// drag handles on touch, press-drag and double/triple click with a
    /// mouse, the copy and select-all chords, and the selection toolbar
    /// offering Copy and Select all (see the module docs' *Selection*).
    /// `false`, the default, keeps the block inert to selection at no cost.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }
}

impl<State: 'static> View<State> for RichTextView {
    type Element = RichTextWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> RichTextWidget {
        RichTextWidget {
            view: self.clone(),
            layout: None,
            laid_out_max_width: None,
            inspect: None,
            laid_out_span_styles: Vec::new(),
            laid_out_locale_stamp: LocaleStamp::default(),
            fragments: Vec::new(),
            links: link_runs(&self.spans),
            on_link: self.on_link.install::<State>(),
            press: None,
            select: self.selectable.then(|| Box::new(Selection::new())),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut RichTextWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures are not comparable: reinstall the adapter every time.
        element.on_link = self.on_link.install::<State>();
        let mut flags = ChangeFlags::NONE;
        if prev != self {
            let same_text = prev
                .spans
                .iter()
                .map(|s| &s.content)
                .eq(self.spans.iter().map(|s| &s.content));
            element.view = self.clone();
            element.layout = None;
            element.fragments.clear();
            element.links = link_runs(&self.spans);
            element.press = None;
            match (self.selectable, element.select.as_mut()) {
                (true, None) => element.select = Some(Box::new(Selection::new())),
                // Offsets into another text mean nothing: start over.
                (true, Some(select)) if !same_text => select.text_replaced(),
                (true, Some(_)) => {}
                (false, Some(select)) => {
                    select.dispose(ctx);
                    element.select = None;
                }
                (false, None) => {}
            }
            flags = ChangeFlags::LAYOUT;
        }
        // The toolbar pod mounts and drops here, the one pass with a
        // `BuildCtx`, even when the view is unchanged.
        if element.select.is_some() {
            let actions = element.toolbar_actions();
            if let Some(select) = element.select.as_mut() {
                flags |= select.sync_toolbar(actions, ctx);
            }
        }
        flags
    }

    fn teardown(&self, element: &mut RichTextWidget, ctx: &mut BuildCtx<'_>) {
        if let Some(select) = element.select.as_mut() {
            select.dispose(ctx);
        }
    }
}

/// One link: consecutive spans sharing a [`SpanView::link`] id.
struct LinkRun {
    id: String,
    /// The link's text, its accessible label.
    label: String,
    spans: Range<usize>,
}

/// The links of `spans`, in order.
fn link_runs(spans: &[SpanView]) -> Vec<LinkRun> {
    let mut runs: Vec<LinkRun> = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let Some(id) = &span.link else { continue };
        match runs.last_mut() {
            Some(last) if last.spans.end == index && last.id == *id => {
                last.label.push_str(&span.content);
                last.spans.end = index + 1;
            }
            _ => runs.push(LinkRun {
                id: id.clone(),
                label: span.content.clone(),
                spans: index..index + 1,
            }),
        }
    }
    runs
}

/// One line's share of a marked span group, in layout coordinates: the
/// glyphs' pen extent across, the line box down.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fragment {
    /// The first span of the group, whose marks every span in it shares.
    span: usize,
    rect: Rect,
}

/// A press that began on link `link` at `start`.
struct LinkPress {
    link: usize,
    start: Point,
}

/// Retained widget for [`RichTextView`]: caches the shaped [`TextLayout`]
/// produced during layout for reuse in paint.
pub struct RichTextWidget {
    view: RichTextView,
    /// `None` until the first layout pass, or after a view change.
    layout: Option<TextLayout>,
    /// The width the cached layout was broken at.
    laid_out_max_width: Option<f32>,
    /// Layout report from the last real layout, served by `inspect_detail`.
    inspect: Option<TextInspect>,
    /// Every span's effective style, so a bare theme swap or text-scale
    /// change re-shapes without a view change. Kept before grouping:
    /// grouping depends on the theme-resolved styles, so two layouts with
    /// equal group styles can still split the spans differently.
    laid_out_span_styles: Vec<TextStyle>,
    /// The locale stamp read just before shaping.
    laid_out_locale_stamp: LocaleStamp,
    /// Every marked group's line fragments from the last layout, in line
    /// order; empty when no span has a background or link.
    fragments: Vec<Fragment>,
    links: Vec<LinkRun>,
    on_link: Option<ErasedArgCallback<String>>,
    press: Option<LinkPress>,
    /// The selection machinery, present only for a
    /// [`RichTextView::selectable`] block.
    select: Option<Box<Selection>>,
}

impl RichTextWidget {
    /// The link whose glyphs lie under the local point `at`, if any.
    fn link_at(&self, at: Point) -> Option<usize> {
        self.fragments
            .iter()
            .filter(|f| f.rect.contains(at))
            .find_map(|f| self.link_of(f.span))
    }

    /// The link span `span` belongs to.
    fn link_of(&self, span: usize) -> Option<usize> {
        self.links.iter().position(|l| l.spans.contains(&span))
    }

    /// Every span's effective style, with the paragraph knobs applied.
    fn effective_styles(&self, theme: Option<&Theme>) -> Vec<TextStyle> {
        self.view
            .spans
            .iter()
            .map(|span| {
                let mut style = span.effective_style(theme);
                if let Some(align) = self.view.align {
                    style.align = align;
                }
                if let Some(wrap) = self.view.overflow_wrap {
                    style.overflow_wrap = wrap;
                }
                style
            })
            .collect()
    }
}

/// A run of adjacent spans shaped as one: the index of the span whose style
/// they all share, and the spans' range.
struct SpanGroup {
    style: usize,
    spans: std::ops::Range<usize>,
}

/// What decides whether two adjacent spans may be shaped as one group: equal
/// effective style, background and link.
struct GroupKey<'a> {
    style: &'a TextStyle,
    span: &'a SpanView,
}

impl PartialEq for GroupKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.style == other.style && self.span.same_marks(other.span)
    }
}

/// Groups the non-empty spans (`contents`) for shaping: empty spans are
/// dropped, adjacent spans with equal `keys` merge (the widget keys on
/// effective style, background and link), and past `limit` groups
/// the excess folds into the last kept one (its text takes that group's
/// style) instead of overflowing the shaper's span index.
///
/// Text with no non-empty span keeps its first span so the empty layout is
/// still sized by that span's style.
fn group_spans<'a, K: PartialEq>(
    contents: impl ExactSizeIterator<Item = &'a str>,
    keys: &[K],
    limit: usize,
) -> Vec<SpanGroup> {
    let mut groups: Vec<SpanGroup> = Vec::new();
    let count = contents.len();
    for (index, content) in contents.enumerate() {
        if content.is_empty() {
            continue;
        }
        let full = groups.len() >= limit;
        match groups.last_mut() {
            Some(last) if keys[last.style] == keys[index] => last.spans.end = index + 1,
            Some(last) if full => last.spans.end = index + 1,
            _ => groups.push(SpanGroup {
                style: index,
                spans: index..index + 1,
            }),
        }
    }
    if groups.is_empty() && count > 0 {
        groups.push(SpanGroup {
            style: 0,
            spans: 0..1,
        });
    }
    groups
}

impl Widget for RichTextWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if let Some(select) = self.select.as_mut() {
            // The floated toolbar lays out against the window, every pass,
            // and the builder is handed the window `View::rebuild` cannot see.
            select.window_size = ctx.window_size();
            select.toolbar.layout(ctx);
        }
        let max_width = {
            let w = bc.max().width;
            if w.is_finite() { Some(w as f32) } else { None }
        };
        let all_styles = self.effective_styles(Theme::from_layout_ctx(ctx));
        let locale_stamp = LocaleStamp::for_style(None);
        if let Some(cached) = &self.layout
            && self.laid_out_max_width == max_width
            && self.laid_out_span_styles == all_styles
            && self.laid_out_locale_stamp == locale_stamp
        {
            return bc.constrain(cached.size());
        }

        let view_spans = &self.view.spans;
        let keys: Vec<GroupKey<'_>> = view_spans
            .iter()
            .zip(&all_styles)
            .map(|(span, style)| GroupKey { style, span })
            .collect();
        let groups = group_spans(
            view_spans.iter().map(|s| s.content.as_str()),
            &keys,
            usize::from(u16::MAX),
        );
        let styles: Vec<TextStyle> = groups.iter().map(|g| all_styles[g.style].clone()).collect();
        let texts: Vec<String> = groups
            .iter()
            .map(|g| {
                self.view.spans[g.spans.clone()]
                    .iter()
                    .map(|s| s.content.as_str())
                    .collect()
            })
            .collect();
        let spans: Vec<TextSpan<'_>> = texts
            .iter()
            .zip(&styles)
            .map(|(text, style)| TextSpan::new(text, style))
            .collect();
        let text_ctx = ctx.text_context::<TextContext>();
        let layout = text_ctx.layout_rich_bounded(
            &spans,
            max_width,
            self.view.max_lines,
            self.view.overflow,
        );
        let joined: String = texts.concat();
        let mut start = 0;
        let marked: Vec<(Range<usize>, usize)> = groups
            .iter()
            .zip(&texts)
            .filter_map(|(g, text)| {
                let range = start..start + text.len();
                start = range.end;
                self.view.spans[g.style]
                    .is_marked()
                    .then_some((range, g.style))
            })
            .collect();
        let fragments = marked_fragments(&layout, &joined, &marked);
        let size = bc.constrain(layout.size());
        self.inspect = Some(text_inspect(&joined, &layout, self.view.max_lines));
        self.layout = Some(layout);
        self.fragments = fragments;
        self.laid_out_max_width = max_width;
        self.laid_out_span_styles = all_styles;
        self.laid_out_locale_stamp = locale_stamp;
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let mut select = self.select.take();
        if let Some(select) = select.as_deref_mut() {
            select.before_paint(ctx);
        }
        if let Some(layout) = &self.layout {
            let origin = ctx.origin();
            let theme = Theme::from_paint_ctx(ctx);
            for fragment in &self.fragments {
                let Some(color) = self.view.spans[fragment.span].background_color(theme) else {
                    continue;
                };
                let rect = fragment.rect.inflate(BG_PAD_X, 0.0);
                let radius = theme.map_or(BG_RADIUS, |t| t.shape.extra_small);
                let radius = ShapeScale::resolve(radius, rect.width(), rect.height());
                scene.fill_rounded_rect(
                    origin + rect.origin().to_vec2(),
                    rect.size(),
                    radius,
                    color,
                );
            }
            let handles = select.as_deref().map(|select| {
                if select.shows_selection() {
                    // Over the span backgrounds, under the glyphs.
                    let color = selection_color(theme);
                    for rect in layout.range_rects(select.range.clone()) {
                        scene.fill_rect(origin + rect.origin().to_vec2(), rect.size(), color);
                    }
                }
                handle_color(theme)
            });
            paint_layout(layout, origin, scene);
            if let (Some(select), Some(handles)) = (select.as_deref_mut(), handles) {
                self.paint_selection_chrome(select, ctx, scene, handles);
            }
        }
        self.select = select;
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let Some(mut select) = self.select.take() {
            let result = self.select_event(&mut select, ctx, event);
            self.select = Some(select);
            return result;
        }
        if self.on_link.is_none() || self.links.is_empty() {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let within_slop = |press: &LinkPress| (p.position - press.start).hypot() <= TOUCH_SLOP;
        match p.phase {
            PointerPhase::Down => {
                self.press = None;
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(link) = self.link_at(p.position) else {
                    return EventResult::Ignored;
                };
                self.press = Some(LinkPress {
                    link,
                    start: p.position,
                });
                ctx.capture_pointer();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.press.as_ref().is_some_and(|press| !within_slop(press)) {
                    // It became a drag: let it go without firing.
                    self.press = None;
                }
                if self.link_at(p.position).is_some() {
                    ctx.set_cursor(CursorIcon::Pointer);
                }
                if self.press.is_some() {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up => {
                let Some(press) = self.press.take() else {
                    return EventResult::Ignored;
                };
                if presses(p)
                    && within_slop(&press)
                    && self.link_at(p.position) == Some(press.link)
                    && let Some(on_link) = self.on_link.as_mut()
                {
                    on_link(ctx, self.links[press.link].id.clone());
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.press.take().is_some() {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
        }
    }

    fn inspect_detail(&self) -> Option<InspectDetail> {
        self.inspect.clone().map(InspectDetail::Text)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let value: String = self.view.spans.iter().map(|s| s.content.as_str()).collect();
        // A selectable block's clipboard verbs, as `TextInput` publishes its
        // own: custom actions on the paragraph node, only the enabled ones.
        let verbs: Vec<CustomAction> = self
            .select
            .as_deref()
            .map(|select| {
                let actions = self.toolbar_actions_of(select);
                [
                    (A11Y_COPY_ID, "Copy", actions.copy),
                    (A11Y_SELECT_ALL_ID, "Select all", actions.select_all),
                ]
                .into_iter()
                .filter(|(_, _, enabled)| *enabled)
                .map(|(id, description, _)| CustomAction {
                    id,
                    description: description.into(),
                })
                .collect()
            })
            .unwrap_or_default();
        let focused = self.select.as_deref().is_some_and(|s| s.focused) && ctx.has_focus();
        let describe = |node: &mut frust_core::accesskit::Node| {
            node.set_value(value);
            if !verbs.is_empty() {
                node.add_action(Action::CustomAction);
                node.set_custom_actions(verbs);
            }
        };
        if self.links.is_empty() {
            let id = ctx.push_node(Role::Label, describe);
            if focused {
                ctx.set_focused(id);
            }
            return;
        }
        let origin = ctx.origin();
        let id = ctx.push_container(Role::Label, describe, |ctx| {
            for (index, link) in self.links.iter().enumerate() {
                // A link cut off by `max_lines` has no glyphs to activate.
                let Some(first) = self
                    .fragments
                    .iter()
                    .find(|f| self.link_of(f.span) == Some(index))
                else {
                    continue;
                };
                // The first fragment's bounds, so a Click synthesized at
                // their centre lands on this link's glyphs.
                let bounds = first.rect + origin.to_vec2();
                ctx.push_node(Role::Link, |node| {
                    node.set_label(link.label.as_str());
                    node.set_bounds(AccessRect {
                        x0: bounds.x0,
                        y0: bounds.y0,
                        x1: bounds.x1,
                        y1: bounds.y1,
                    });
                    if self.on_link.is_some() {
                        node.add_action(Action::Click);
                    }
                });
            }
        });
        if focused {
            ctx.set_focused(id);
        }
    }

    /// A selectable block is a focus stop only while it holds (or is asking
    /// for) the focus its selection lives in: never a Tab stop at rest, but a
    /// stop the root's walk reaches when its [`FocusHandle`] asks.
    fn is_focusable(&self) -> bool {
        self.select.as_deref().is_some_and(|s| s.focused)
    }
}

/// What the live press of a selectable block is doing — what the next
/// `Move` may do and what its `Up` means.
#[derive(Clone, Debug, Default, PartialEq)]
enum SelectPress {
    /// No press.
    #[default]
    None,
    /// A primary press still within [`TOUCH_SLOP`] of `at`: a tap so far,
    /// and on touch a long-press candidate. `link` is the link it landed on,
    /// `in_selection` whether the toolbar was up when it landed inside the
    /// selection (the toggle's memory).
    Tap {
        at: Point,
        touch: bool,
        link: Option<usize>,
        in_selection: Option<bool>,
    },
    /// A mouse press-drag selecting by grapheme from `anchor`.
    Drag { anchor: usize },
    /// A selected `word` (a fired long-press, or a double-click) whose press
    /// is still down: within the slop of `at` it holds still, past it the
    /// selection extends from the word.
    Word {
        at: Point,
        word: Range<usize>,
        dragging: bool,
        hold: bool,
    },
    /// A handle drag: the other end stays at `fixed`, the dragged end
    /// follows the pointer offset by `grab`.
    Handle { fixed: usize, grab: Vec2 },
    /// A press that is no selection gesture (a touch drag the parent may
    /// scroll with, or a triple-click's rest): ignored until it ends.
    Pass,
}

/// The selection state and machinery of a [`RichTextView::selectable`]
/// block: the selected range, its focus, the live press, the handles, the
/// scroll vetoes a gesture raises, and the floated toolbar.
struct Selection {
    /// The selected byte range of the block's text; empty for none.
    range: Range<usize>,
    /// The block holds focus, or has asked for it (see `focus_pending`).
    focused: bool,
    /// A focus request through `focus` is on its way: one paint without
    /// focus is tolerated before the selection is dropped.
    focus_pending: bool,
    /// Bound every paint, so a gesture that starts a selection outside a
    /// `Down` can still move focus here.
    focus: FocusHandle,
    /// Whether the selection shows drag handles (a touch selection).
    handles: bool,
    press: SelectPress,
    hold: Option<HoldTimer>,
    /// The enclosing scroll surfaces' veto cells, captured on the `Down`.
    vetoes: Vec<Rc<Cell<bool>>>,
    /// This block raised `vetoes` and has not lowered them yet.
    raised: bool,
    /// The last completed click: where, the paint clock when the first paint
    /// after it dated it (`None` until then), and how many clicks it
    /// completed — the double/triple-click window.
    last_click: Option<(Point, Option<FrameTime>, u8)>,
    /// The input time of the most recent paint: the event pass's clock.
    last_input_time: FrameTime,
    toolbar: OverlaySlot<()>,
    toolbar_view: Option<AnyView<()>>,
    toolbar_view_actions: Option<SelectionToolbarActions>,
    toolbar_open: bool,
    /// The selection's bounding box in window space as of the last paint.
    toolbar_anchor: Rect,
    window_size: Size,
    /// The light-dismiss notice closed the toolbar for a press about to land
    /// here (see `TextInput`'s field of the same name).
    outside_press_dismissed: bool,
}

impl Selection {
    fn new() -> Self {
        Self {
            range: 0..0,
            focused: false,
            focus_pending: false,
            focus: FocusHandle::new(),
            handles: false,
            press: SelectPress::None,
            hold: None,
            vetoes: Vec::new(),
            raised: false,
            last_click: None,
            last_input_time: FrameTime::ZERO,
            toolbar: new_toolbar_slot(),
            toolbar_view: None,
            toolbar_view_actions: None,
            toolbar_open: false,
            toolbar_anchor: Rect::ZERO,
            window_size: Size::ZERO,
            outside_press_dismissed: false,
        }
    }

    /// Whether the highlight paints: a selection the block holds focus for.
    fn shows_selection(&self) -> bool {
        self.focused && !self.range.is_empty()
    }

    /// Drop the selection and what hangs off it (handles, toolbar).
    fn clear(&mut self) {
        self.range = 0..0;
        self.handles = false;
        self.toolbar_open = false;
    }

    /// The text was replaced: offsets held by the live press, the selection
    /// and the click window mean nothing in the new text. The press ends (its
    /// scroll vetoes are lowered), and a block left holding focus with
    /// nothing selected gives it back through its handle.
    fn text_replaced(&mut self) {
        self.end_press();
        self.clear();
        self.last_click = None;
        if self.focused {
            self.focus.release();
            self.lose_focus();
        }
    }

    /// The block lost focus: its selection goes with it.
    fn lose_focus(&mut self) {
        self.clear();
        self.focused = false;
        self.focus_pending = false;
        self.outside_press_dismissed = false;
    }

    /// Give focus up and drop the selection.
    fn release(&mut self, ctx: &mut EventCtx) {
        if self.focused {
            ctx.release_focus();
            self.focus.release();
        }
        self.lose_focus();
        ctx.request_redraw();
    }

    /// Select `range`, taking focus for it when the block does not hold it.
    fn select(&mut self, ctx: &mut EventCtx, range: Range<usize>) {
        if self.range != range {
            self.range = range;
            ctx.request_redraw();
        }
        if !self.range.is_empty() && !self.focused {
            // Outside a `Down` the root honours no `request_focus`, so the
            // claim goes through the bound handle (granted at the pass end).
            self.focused = true;
            self.focus_pending = true;
            self.focus.request();
        }
    }

    fn hide_toolbar(&mut self, ctx: &mut EventCtx) {
        if self.toolbar_open {
            self.toolbar_open = false;
            ctx.request_redraw();
        }
    }

    /// Raise the captured vetoes: this gesture is the block's, and no
    /// enclosing scroll surface may take it over.
    fn raise_vetoes(&mut self) {
        for veto in &self.vetoes {
            veto.set(true);
        }
        self.raised = !self.vetoes.is_empty();
    }

    /// Lower what this block raised; a gesture that raised nothing writes no
    /// cell.
    fn lower_vetoes(&mut self) {
        if std::mem::take(&mut self.raised) {
            for veto in &self.vetoes {
                veto.set(false);
            }
        }
    }

    /// End the press, keeping the selection.
    fn end_press(&mut self) {
        self.press = SelectPress::None;
        self.hold = None;
        self.lower_vetoes();
        self.vetoes.clear();
    }

    /// The paint-side half: the clock, the long-press timer, the focus
    /// handle binding and the focus convergence (the pod's recorded focus is
    /// authoritative; a blur this block never saw drops the selection here).
    fn before_paint(&mut self, ctx: &mut PaintCtx) {
        let now = ctx.input_time();
        self.last_input_time = now;
        // The click window is measured on this paint clock, which only
        // advances while something paints: ask for plain frames until the
        // window has passed (the first paint after the click dates it), then
        // drop the click so an idle block asks for none.
        if let Some((_, time, _)) = self.last_click.as_mut() {
            let dated = *time.get_or_insert(now);
            if now.saturating_sub(dated).as_secs_f64() * 1000.0 > DOUBLE_TAP_MS {
                self.last_click = None;
            } else {
                ctx.request_frame();
            }
        }
        if matches!(self.press, SelectPress::Tap { touch: true, .. })
            && let Some(hold) = self.hold.as_mut()
        {
            hold.tick(ctx, now);
        }
        ctx.bind_focus_handle(&self.focus);
        if ctx.has_focus() {
            self.focused = true;
            self.focus_pending = false;
        } else if self.focused {
            if std::mem::take(&mut self.focus_pending) {
                // The grant may land with the next pass; one paint's grace.
            } else {
                self.lose_focus();
            }
        }
    }

    /// Mount, reconcile or drop the toolbar pod for the verbs `actions`, as
    /// `TextInput`'s `sync_toolbar` does: keyed on the action set alone.
    fn sync_toolbar(
        &mut self,
        actions: SelectionToolbarActions,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let wanted = (self.toolbar_open
            && self.focused
            && selection_toolbar_policy() == SelectionToolbarPolicy::Framework)
            .then_some(actions);
        if wanted == self.toolbar_view_actions {
            return ChangeFlags::NONE;
        }
        let next = wanted.and_then(|actions| {
            selection_toolbar_builder().map(|builder| {
                builder(
                    &SelectionToolbarRequest {
                        anchor: self.toolbar_anchor,
                        actions,
                        present_menu: true,
                    },
                    self.window_size,
                )
            })
        });
        let prev = self.toolbar_view.take();
        let flags = self.toolbar.rebuild(prev.as_ref(), next.as_ref(), ctx);
        self.toolbar_view = next;
        self.toolbar_view_actions = wanted;
        flags
    }

    /// The block is going away (or stops being selectable): tear the toolbar
    /// pod down and lower any veto a live press raised.
    fn dispose(&mut self, ctx: &mut BuildCtx<'_>) {
        let prev = self.toolbar_view.take();
        self.toolbar.rebuild(prev.as_ref(), None, ctx);
        self.toolbar_view_actions = None;
        self.end_press();
        self.clear();
    }
}

/// Whether `pos` (widget-local) lies within a `size`-sized block.
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

impl RichTextWidget {
    /// The block's text, as laid out.
    fn joined(&self) -> String {
        self.view.spans.iter().map(|s| s.content.as_str()).collect()
    }

    fn text_len(&self) -> usize {
        self.view.spans.iter().map(|s| s.content.len()).sum()
    }

    /// The caret position nearest the local point `at`.
    fn index_at(&self, at: Point) -> Option<usize> {
        Some(self.layout.as_ref()?.position_at_point(at).index)
    }

    /// The word under the local point `at`.
    fn word_at(&self, at: Point) -> Range<usize> {
        self.layout.as_ref().map_or(0..0, |layout| {
            layout.word_range_at(layout.position_at_point(at))
        })
    }

    /// The caret rects at `range`'s start and end, where its handles stand.
    fn end_carets(&self, range: &Range<usize>) -> Option<[Rect; 2]> {
        let layout = self.layout.as_ref()?;
        Some([
            layout.caret_rect(
                TextPosition::new(range.start, TextAffinity::Downstream),
                0.0,
            ),
            layout.caret_rect(TextPosition::new(range.end, TextAffinity::Upstream), 0.0),
        ])
    }

    fn in_selection(&self, select: &Selection, at: Point) -> bool {
        self.layout.as_ref().is_some_and(|layout| {
            layout
                .range_rects(select.range.clone())
                .iter()
                .any(|r| r.contains(at))
        })
    }

    /// The handle a press at `at` grabs, as the end that stays put and the
    /// offset from the press to the dragged end's caret.
    fn handle_at(&self, select: &Selection, at: Point) -> Option<(usize, Vec2)> {
        if !(select.handles && select.shows_selection()) {
            return None;
        }
        let [start, end] = self.end_carets(&select.range)?;
        let grab = |caret: Rect| Point::new(caret.x0, (caret.y0 + caret.y1) / 2.0) - at;
        match (handle_hit(start, at), handle_hit(end, at)) {
            (Some(a), Some(b)) if b < a => Some((select.range.start, grab(end))),
            (Some(_), _) => Some((select.range.end, grab(start))),
            (None, Some(_)) => Some((select.range.start, grab(end))),
            (None, None) => None,
        }
    }

    /// The verbs the selection offers: Copy and Select all, never Cut or
    /// Paste.
    fn toolbar_actions_of(&self, select: &Selection) -> SelectionToolbarActions {
        let len = self.text_len();
        SelectionToolbarActions {
            copy: !select.range.is_empty(),
            cut: false,
            paste: false,
            select_all: len > 0 && select.range.len() < len,
        }
    }

    fn toolbar_actions(&self) -> SelectionToolbarActions {
        self.select
            .as_deref()
            .map_or_else(SelectionToolbarActions::default, |s| {
                self.toolbar_actions_of(s)
            })
    }

    /// Where the toolbar hangs, block-local: the selection's bounding box, or
    /// the caret at its start when it is empty.
    fn selection_anchor(&self, select: &Selection) -> Rect {
        let Some(layout) = &self.layout else {
            return Rect::ZERO;
        };
        layout
            .range_rects(select.range.clone())
            .into_iter()
            .reduce(|a, b| a.union(b))
            .unwrap_or_else(|| {
                layout.caret_rect(
                    TextPosition::new(select.range.start, TextAffinity::Downstream),
                    0.0,
                )
            })
    }

    /// Write the selection to the pass's clipboard slot.
    fn copy(&self, select: &Selection, ctx: &mut EventCtx) {
        if select.range.is_empty() {
            return;
        }
        if let Some(text) = self.joined().get(select.range.clone()) {
            ctx.write_clipboard(text.to_owned());
        }
    }

    /// Apply a clipboard verb: Copy and Select all; Cut and Paste are
    /// answered with nothing, the block being read-only. A verb spent puts
    /// the toolbar away.
    fn apply_verb(&self, select: &mut Selection, ctx: &mut EventCtx, verb: ClipboardVerb) {
        match verb {
            ClipboardVerb::Copy => self.copy(select, ctx),
            ClipboardVerb::SelectAll => select.select(ctx, 0..self.text_len()),
            ClipboardVerb::Cut | ClipboardVerb::Paste => {}
        }
        select.hide_toolbar(ctx);
    }

    fn apply_command(&self, select: &mut Selection, ctx: &mut EventCtx, cmd: &EditCommand) {
        let verb = match cmd {
            EditCommand::Copy => ClipboardVerb::Copy,
            EditCommand::Cut => ClipboardVerb::Cut,
            EditCommand::Paste(_) => ClipboardVerb::Paste,
            EditCommand::SelectAll => ClipboardVerb::SelectAll,
        };
        self.apply_verb(select, ctx, verb);
    }

    /// Fire a long-press whose threshold a paint observed: select the word
    /// under the press, show the handles and the toolbar, and keep the
    /// gesture from the parent scroll. Reports whether it fired.
    fn fire_hold(&self, select: &mut Selection, ctx: &mut EventCtx) -> bool {
        if !select.hold.is_some_and(|hold| hold.elapsed()) {
            return false;
        }
        select.hold = None;
        let SelectPress::Tap { at, .. } = select.press else {
            return false;
        };
        let word = self.word_at(at);
        select.press = SelectPress::Word {
            at,
            word: word.clone(),
            dragging: false,
            hold: true,
        };
        if word.is_empty() {
            return true;
        }
        select.select(ctx, word);
        select.handles = true;
        select.toolbar_open = true;
        select.raise_vetoes();
        ctx.request_redraw();
        true
    }

    /// The selection a pointer at `at` makes from a held `word`: the word
    /// itself while the pointer is inside it, else from its far end to the
    /// pointer.
    fn extend_word(&self, word: &Range<usize>, at: Point) -> Range<usize> {
        let Some(index) = self.index_at(at) else {
            return word.clone();
        };
        if index >= word.end {
            word.start..index
        } else if index <= word.start {
            index..word.end
        } else {
            word.clone()
        }
    }

    /// The selectable block's whole event handling (see the module docs'
    /// *Selection*); a link tap is part of it.
    fn select_event(
        &mut self,
        select: &mut Selection,
        ctx: &mut EventCtx,
        event: &InputEvent,
    ) -> EventResult {
        // A blur this block was never told about (a sibling took the focus).
        if select.focused && !select.focus_pending && !ctx.has_focus() {
            select.lose_focus();
            ctx.request_redraw();
        }
        // The floated toolbar gets first refusal, and its verbs land here in
        // the pass it dispatched them in.
        if let Some(result) = select.toolbar.event(ctx, event, &mut ()) {
            let commands = ctx.take_edit_commands();
            if select.focused {
                for cmd in &commands {
                    self.apply_command(select, ctx, cmd);
                }
            }
            if select.toolbar.take_outside_down() {
                select.outside_press_dismissed = select.toolbar_open;
                select.hide_toolbar(ctx);
            }
            return result;
        }
        match event {
            InputEvent::Pointer(p) => self.select_pointer(select, ctx, p),
            InputEvent::Key(key) => {
                if !ctx.has_focus() || !select.focused {
                    return EventResult::Ignored;
                }
                if let Some(verb) = clipboard_verb(&key.key, key.modifiers) {
                    self.apply_verb(select, ctx, verb);
                    return EventResult::Handled;
                }
                if key.key == Key::Named(NamedKey::Escape) {
                    select.release(ctx);
                    let m = key.modifiers;
                    // A bare Escape is declined, so a modal around the block
                    // still closes on the same press.
                    return if m.shift || m.ctrl || m.alt || m.meta {
                        EventResult::Handled
                    } else {
                        EventResult::Ignored
                    };
                }
                EventResult::Ignored
            }
            InputEvent::EditCommand(cmd) => {
                if !ctx.has_focus() || !select.focused {
                    return EventResult::Ignored;
                }
                self.apply_command(select, ctx, cmd);
                EventResult::Handled
            }
            // The anchor moved out from under the bar; the selection stays.
            InputEvent::Scroll { .. } => {
                select.hide_toolbar(ctx);
                EventResult::Ignored
            }
            InputEvent::Housekeeping => {
                self.fire_hold(select, ctx);
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }

    fn select_pointer(
        &mut self,
        select: &mut Selection,
        ctx: &mut EventCtx,
        p: &frust_core::PointerEvent,
    ) -> EventResult {
        let within_slop = |at: Point| (p.position - at).hypot() <= TOUCH_SLOP;
        match p.phase {
            PointerPhase::Down => {
                select.end_press();
                // A handle stands at a selection end, its knob below the line.
                if let Some((fixed, grab)) = self.handle_at(select, p.position) {
                    // The block holds the focus its handles hang from: keep it,
                    // or the root arms a blur and a handle gesture that travels
                    // less than a touch slop ends as a tap that drops the
                    // selection.
                    ctx.keep_focus();
                    select.press = SelectPress::Handle { fixed, grab };
                    select.vetoes = crate::scroll::ambient_scroll_vetoes();
                    select.raise_vetoes();
                    select.toolbar_open = false;
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if !inside(p.position, ctx.size()) || self.layout.is_none() {
                    // A press outside that still reaches the block (it is the
                    // root) is a blur.
                    if select.focused {
                        select.release(ctx);
                    }
                    return EventResult::Ignored;
                }
                // Keep another widget's session: only a selecting gesture
                // moves focus here. With none live, claim it now — a `Down`
                // is the one pointer phase whose claim the root honours.
                ctx.keep_focus();
                if !ctx.press_keeps_focus() {
                    ctx.request_focus();
                    select.focused = true;
                    select.focus_pending = true;
                }
                let was_open =
                    select.toolbar_open || std::mem::take(&mut select.outside_press_dismissed);
                select.toolbar_open = false;
                if !presses(p) {
                    // The desktop context-menu gesture: the toolbar over
                    // whatever is selected.
                    if select.focused {
                        select.toolbar_open = !was_open;
                    }
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                let touch = ctx.pointer_id().source == PointerSource::Touch;
                select.vetoes = crate::scroll::ambient_scroll_vetoes();
                if !touch {
                    // A mouse drag on text selects; it never scrolls the parent.
                    select.raise_vetoes();
                }
                ctx.capture_pointer();
                ctx.request_redraw();
                let clicks = match select.last_click {
                    Some((at, time, count))
                        if !touch
                            && within_slop(at)
                            && time.is_none_or(|time| {
                                select.last_input_time.saturating_sub(time).as_secs_f64() * 1000.0
                                    <= DOUBLE_TAP_MS
                            }) =>
                    {
                        count + 1
                    }
                    _ => 1,
                };
                select.last_click = None;
                if clicks == 2 {
                    let word = self.word_at(p.position);
                    select.handles = false;
                    select.select(ctx, word.clone());
                    select.press = SelectPress::Word {
                        at: p.position,
                        word,
                        dragging: false,
                        hold: false,
                    };
                    return EventResult::Handled;
                }
                if clicks >= 3 {
                    select.handles = false;
                    select.select(ctx, 0..self.text_len());
                    select.press = SelectPress::Pass;
                    return EventResult::Handled;
                }
                let link = self.on_link.as_ref().and_then(|_| self.link_at(p.position));
                let in_selection = (select.shows_selection()
                    && self.in_selection(select, p.position))
                .then_some(was_open);
                select.press = SelectPress::Tap {
                    at: p.position,
                    touch,
                    link,
                    in_selection,
                };
                if touch {
                    select.hold = Some(HoldTimer::default());
                }
                EventResult::Handled
            }
            PointerPhase::Move => {
                let press = select.press.clone();
                match press {
                    SelectPress::None => {
                        if inside(p.position, ctx.size()) {
                            ctx.set_cursor(if self.link_at(p.position).is_some() {
                                CursorIcon::Pointer
                            } else {
                                CursorIcon::Text
                            });
                        }
                        return EventResult::Ignored;
                    }
                    SelectPress::Tap { at, touch, .. } => {
                        if within_slop(at) {
                            self.fire_hold(select, ctx);
                            return EventResult::Handled;
                        }
                        select.hold = None;
                        if touch {
                            // Not a selection gesture: a parent scroll takes
                            // it, or nothing does.
                            select.press = SelectPress::Pass;
                        } else if let Some(anchor) = self.index_at(at) {
                            select.handles = false;
                            select.press = SelectPress::Drag { anchor };
                            if let Some(index) = self.index_at(p.position) {
                                select.select(ctx, anchor.min(index)..anchor.max(index));
                            }
                        }
                    }
                    SelectPress::Drag { anchor } => {
                        if let Some(index) = self.index_at(p.position) {
                            select.select(ctx, anchor.min(index)..anchor.max(index));
                        }
                    }
                    SelectPress::Word {
                        at,
                        word,
                        dragging,
                        hold,
                    } => {
                        if !dragging && within_slop(at) {
                            return EventResult::Handled;
                        }
                        let range = self.extend_word(&word, p.position);
                        select.press = SelectPress::Word {
                            at,
                            word,
                            dragging: true,
                            hold,
                        };
                        select.select(ctx, range);
                    }
                    SelectPress::Handle { fixed, grab } => {
                        if let Some(index) = self.index_at(p.position + grab)
                            && index != fixed
                        {
                            select.select(ctx, fixed.min(index)..fixed.max(index));
                        }
                    }
                    SelectPress::Pass => {}
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let press = std::mem::take(&mut select.press);
                // A hold whose threshold elapsed with no pass to fire it on
                // fires now, and is no tap.
                if let SelectPress::Tap { .. } = press {
                    select.press = press.clone();
                    if self.fire_hold(select, ctx) {
                        select.end_press();
                        // A hold that found no word selects nothing: the
                        // focus its `Down` claimed goes back.
                        if select.range.is_empty() && select.focused {
                            select.release(ctx);
                        }
                        return EventResult::Handled;
                    }
                    select.press = SelectPress::None;
                }
                match press {
                    SelectPress::None => return EventResult::Ignored,
                    SelectPress::Tap {
                        at,
                        touch,
                        link,
                        in_selection,
                    } => {
                        if let Some(link) = link
                            && within_slop(at)
                            && presses(p)
                            && self.link_at(p.position) == Some(link)
                            && let Some(on_link) = self.on_link.as_mut()
                        {
                            on_link(ctx, self.links[link].id.clone());
                        }
                        match in_selection {
                            // A tap on the selection toggles the toolbar.
                            Some(was_open) => select.toolbar_open = !was_open,
                            // A tap anywhere else clears it.
                            None => select.release(ctx),
                        }
                        if !touch {
                            select.last_click = Some((at, None, 1));
                        }
                    }
                    SelectPress::Word { at, hold, .. } => {
                        if !hold {
                            select.last_click = Some((at, None, 2));
                        }
                    }
                    SelectPress::Handle { .. } => select.toolbar_open = true,
                    SelectPress::Drag { .. } | SelectPress::Pass => {}
                }
                select.end_press();
                // A press that ends selecting nothing gives back the focus
                // its `Down` claimed.
                if select.range.is_empty() && select.focused {
                    select.release(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // Never touch the selection on a cancel: the press ends, the
                // selection stays. Only an empty one gives back the focus the
                // press claimed (a parent scroll that took a press over).
                let had = select.press != SelectPress::None;
                select.end_press();
                if select.range.is_empty() && select.focused {
                    select.release(ctx);
                }
                if had {
                    ctx.request_redraw();
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
        }
    }

    /// The handles, the published toolbar request and the floated toolbar,
    /// painted after the glyphs.
    fn paint_selection_chrome(
        &self,
        select: &mut Selection,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        handle: Color,
    ) {
        if !select.focused {
            return;
        }
        let origin = ctx.origin();
        if select.handles
            && select.shows_selection()
            && let Some(carets) = self.end_carets(&select.range)
        {
            for caret in carets {
                paint_handle(scene, origin, caret, handle);
            }
        }
        let local = self.selection_anchor(select);
        select.toolbar_anchor = local + origin.to_vec2();
        ctx.publish_selection_toolbar(SelectionToolbarRequest {
            anchor: select.toolbar_anchor,
            actions: self.toolbar_actions_of(select),
            present_menu: select.toolbar_open,
        });
        if select.toolbar_open && selection_toolbar_policy() == SelectionToolbarPolicy::Framework {
            select.toolbar.set_anchor(OverlayAnchor::Rect(local));
            let size = ctx.size();
            select.toolbar.paint(ctx, size);
        }
    }
}

/// The line fragments of every marked group (`marked` holds each one's byte
/// range in `text`, the layout's source, and its first span), sorted by line
/// and then left edge.
///
/// A group's fragments are the [`TextLayout::range_rects`] of its range with
/// every newline left out (a newline draws nothing, so it gets no fill and no
/// hit area), each widened to its line's box ([`TextLayout::line_bounds`]) so
/// the fills of consecutive lines meet. Text a `max_lines` cut dropped has no
/// fragment.
fn marked_fragments(
    layout: &TextLayout,
    text: &str,
    marked: &[(Range<usize>, usize)],
) -> Vec<Fragment> {
    let mut out: Vec<(usize, Fragment)> = Vec::new();
    for (range, span) in marked {
        let mut piece = range.start;
        // A stale range must not panic: skip what is not a slice of `text`.
        let Some(slice) = text.get(range.clone()) else {
            continue;
        };
        let ends = slice
            .char_indices()
            .filter(|(_, c)| is_newline(*c))
            .map(|(at, c)| (range.start + at, c.len_utf8()))
            .chain([(range.end, 0)]);
        for (end, len) in ends {
            for rect in layout.range_rects(piece..end) {
                let line = line_at(layout, (rect.y0 + rect.y1) / 2.0);
                let (y0, y1) = layout.line_bounds(line).unwrap_or((rect.y0, rect.y1));
                out.push((
                    line,
                    Fragment {
                        span: *span,
                        rect: Rect::new(rect.x0, y0, rect.x1, y1),
                    },
                ));
            }
            piece = end + len;
        }
    }
    out.sort_by(|(la, a), (lb, b)| la.cmp(lb).then(a.rect.x0.total_cmp(&b.rect.x0)));
    out.into_iter().map(|(_, f)| f).collect()
}

/// Whether `c` is a hard line break the line breaker ends a line at.
fn is_newline(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// The line of `layout` whose box holds `y`: the last line whose top is at or
/// above it.
fn line_at(layout: &TextLayout, y: f64) -> usize {
    let lines = layout.line_count();
    let (mut lo, mut hi) = (0, lines.saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if layout.line_bounds(mid).is_some_and(|(top, _)| top <= y) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_scene::GlyphRun;
    use kurbo::Point;
    use peniko::Brush;
    use std::any::Any;

    #[derive(Debug, PartialEq)]
    enum Op {
        Rect(Point, Size, Color),
        Glyphs(Brush, f32),
    }

    #[derive(Default)]
    struct Recorder(Vec<Op>);

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.0.push(Op::Rect(o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.0.push(Op::Glyphs(run.brush, run.font_size));
        }
    }

    fn lay_out(
        view: &RichTextView,
        theme: Option<&Theme>,
        width: f64,
        tcx: &mut TextContext,
    ) -> (RichTextWidget, Size) {
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut 0u64));
        let size = layout_widget(&mut widget, theme, width, tcx);
        (widget, size)
    }

    fn layout_widget(
        widget: &mut RichTextWidget,
        theme: Option<&Theme>,
        width: f64,
        tcx: &mut TextContext,
    ) -> Size {
        let mut lctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        widget.layout(&mut lctx, &BoxConstraints::loose(Size::new(width, 400.0)))
    }

    fn paint(widget: &mut RichTextWidget) -> Vec<Op> {
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(300.0, 400.0));
        widget.paint(&mut pctx, &mut rec);
        rec.0
    }

    fn painted(view: &RichTextView, theme: Option<&Theme>) -> Vec<Op> {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(view, theme, 300.0, &mut tcx);
        paint(&mut widget)
    }

    /// The style of each span group the widget last laid out, regrouped
    /// from the per-span styles it keys reuse on.
    fn group_styles(widget: &RichTextWidget) -> Vec<TextStyle> {
        let styles = &widget.laid_out_span_styles;
        group_spans(
            widget.view.spans.iter().map(|s| s.content.as_str()),
            styles,
            usize::from(u16::MAX),
        )
        .iter()
        .map(|g| styles[g.style].clone())
        .collect()
    }

    fn glyph_colors(ops: &[Op]) -> Vec<Color> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Glyphs(Brush::Solid(c), _) => Some(*c),
                _ => None,
            })
            .collect()
    }

    fn glyph_sizes(ops: &[Op]) -> Vec<f32> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Glyphs(_, s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    fn red() -> Color {
        Color::from_rgb8(200, 20, 20)
    }
    fn green() -> Color {
        Color::from_rgb8(20, 160, 20)
    }
    fn blue() -> Color {
        Color::from_rgb8(20, 20, 200)
    }

    #[test]
    fn three_spans_paint_their_own_colours() {
        let view = rich_text([
            span("one ").color(red()),
            span("two ").color(green()),
            span("three").color(blue()),
        ]);
        let colors = glyph_colors(&painted(&view, None));
        assert_eq!(colors, vec![red(), green(), blue()]);
    }

    #[test]
    fn a_bold_span_shapes_with_its_own_weight() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("plain "), span("bold").weight(FontWeight::BOLD)]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        // The weight is part of the style each span is shaped with; the
        // neighbouring span keeps the default.
        let groups = group_styles(&widget);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].weight, FontWeight::default());
        assert_eq!(groups[1].weight, FontWeight::BOLD);
        assert_eq!(paint(&mut widget).len(), 2, "one run per span");
    }

    #[test]
    fn only_the_underlined_span_gets_an_underline_rect() {
        let plain = painted(&rich_text([span("aaaa "), span("bbbb")]), None);
        assert!(plain.iter().all(|op| !matches!(op, Op::Rect(..))));

        let ops = painted(&rich_text([span("aaaa "), span("bbbb").underline()]), None);
        let rects: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Rect(o, s, _) => Some((*o, *s)),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 1, "{ops:?}");
        let (origin, size) = rects[0];
        // The first span's text occupies the left of the line; the underline
        // starts after it and does not cover the whole paragraph width.
        assert!(origin.x > 0.0, "starts under the second span: {origin:?}");
        assert!(size.width > 0.0 && size.height > 0.0);
        assert!(matches!(ops[0], Op::Rect(..)), "underline paints first");
    }

    #[test]
    fn line_through_paints_after_the_glyph_runs() {
        let ops = painted(
            &rich_text([span("aaaa "), span("bbbb").line_through()]),
            None,
        );
        assert!(matches!(ops.last(), Some(Op::Rect(..))), "{ops:?}");
    }

    fn themed(brightness: frust_theme::Brightness) -> Theme {
        Theme::neutral().with_brightness(brightness)
    }

    #[test]
    fn themed_colour_follows_a_brightness_flip_but_explicit_colour_does_not() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("themed "), span("fixed").color(red())]);
        let light = themed(frust_theme::Brightness::Light);
        let dark = themed(frust_theme::Brightness::Dark);
        assert_ne!(light.scheme().on_surface, dark.scheme().on_surface);

        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, Some(&light), 300.0, &mut tcx);
        let before = glyph_colors(&paint(&mut widget));
        assert_eq!(before, vec![light.scheme().on_surface, red()]);

        layout_widget(&mut widget, Some(&dark), 300.0, &mut tcx);
        let after = glyph_colors(&paint(&mut widget));
        assert_eq!(after, vec![dark.scheme().on_surface, red()]);
    }

    #[test]
    fn semantics_value_is_the_concatenated_text() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut root: frust_core::RenderRoot<(), RichTextView> = frust_core::RenderRoot::new();
        let mut logic = |_: &mut ()| rich_text([span("Hello, "), span("rich "), span("world")]);
        root.rebuild(&mut logic, &mut ());
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(400.0, 200.0), &mut tcx as &mut dyn std::any::Any);
        let update = root.semantics();
        let window = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.root)
            .expect("window node")
            .1
            .clone();
        let label = update
            .nodes
            .iter()
            .find(|(id, _)| *id == window.children()[0])
            .expect("rich text node")
            .1
            .clone();
        assert_eq!(label.role(), frust_core::accesskit::Role::Label);
        assert_eq!(label.value(), Some("Hello, rich world"));
    }

    #[test]
    fn max_lines_with_ellipsis_truncates() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let words = "word ".repeat(40);
        let open = rich_text([span(words.clone()), span("tail").weight(FontWeight::BOLD)]);
        let capped = open.clone().max_lines(1).overflow(TextOverflow::Ellipsis);
        let mut tcx = TextContext::new();
        let (_, full) = lay_out(&open, None, 120.0, &mut tcx);
        let (_, cut) = lay_out(&capped, None, 120.0, &mut tcx);
        assert!(full.height > cut.height * 1.5, "{full:?} vs {cut:?}");
    }

    #[test]
    fn rich_text_reports_flags_and_concatenated_text() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let words = "word ".repeat(40);
        let mut root: frust_core::RenderRoot<(), RichTextView> = frust_core::RenderRoot::new();
        root.rebuild(
            &mut |_: &mut ()| {
                rich_text([span(words.clone()), span("tail").weight(FontWeight::BOLD)])
                    .max_lines(1)
                    .overflow(TextOverflow::Ellipsis)
            },
            &mut (),
        );
        let mut tcx = TextContext::new();
        root.layout_with_text(Size::new(120.0, 200.0), &mut tcx as &mut dyn std::any::Any);
        let Some(Some(InspectDetail::Text(r))) =
            root.inspect().into_iter().map(|n| n.detail).next()
        else {
            panic!("no text detail");
        };
        assert_eq!(r.text, format!("{words}tail"));
        assert_eq!(r.lines, 1);
        assert!(r.did_exceed_max_lines && r.ellipsized, "{r:?}");
    }

    #[test]
    fn text_scale_enlarges_only_default_sized_spans() {
        // A pinned size differing from the default, so the two spans stay
        // separate groups.
        let view = rich_text([span("auto "), span("pinned").size(17.0)]);
        let base = Theme::neutral();
        let authored = base.type_scale.clone();
        let scaled = base.clone().with_text_scale(&authored, 1.5);

        let plain = glyph_sizes(&painted(&view, Some(&base)));
        let big = glyph_sizes(&painted(&view, Some(&scaled)));
        assert_eq!(plain.len(), 2);
        assert_eq!(big.len(), 2);
        assert!((big[0] / plain[0] - 1.5).abs() < 0.1, "{plain:?} {big:?}");
        assert!((big[1] - plain[1]).abs() < 0.01, "explicit size is pinned");
    }

    #[test]
    fn a_text_scale_change_that_regroups_spans_relays_out() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        // At scale 1.0 the default size equals the pinned 16, so the groups
        // are [ab:16, c:24]; at scale 1.5 it is 24 and they are [a:16, bc:24].
        // Both group-style vectors are [S16, S24].
        let view = rich_text([span("a").size(16.0), span("b"), span("c").size(24.0)]);
        let base = Theme::neutral();
        let authored = base.type_scale.clone();
        let scaled = base.clone().with_text_scale(&authored, 1.5);

        let mut tcx = TextContext::new();
        let (mut reused, _) = lay_out(&view, Some(&base), 300.0, &mut tcx);
        let reused_size = layout_widget(&mut reused, Some(&scaled), 300.0, &mut tcx);
        let (mut fresh, fresh_size) = lay_out(&view, Some(&scaled), 300.0, &mut tcx);
        assert_eq!(
            glyph_sizes(&paint(&mut reused)),
            glyph_sizes(&paint(&mut fresh))
        );
        assert_eq!(reused_size, fresh_size, "\"b\" lays out at 24px");
    }

    #[test]
    fn an_unchanged_span_list_reuses_the_layout() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("alpha ").color(red()), span("beta").underline()]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        let shapes = tcx.shape_cache_stats().shapes;
        assert_eq!(shapes, 1);

        // An identical rebuild changes nothing and an extra pass skips the
        // text context entirely.
        let again = view.clone();
        let flags = View::<()>::rebuild(&again, &view, &mut widget, &mut BuildCtx::new(&mut 0u64));
        assert_eq!(flags, ChangeFlags::NONE);
        layout_widget(&mut widget, None, 300.0, &mut tcx);
        let stats = tcx.shape_cache_stats();
        assert_eq!(stats.shapes, 1, "no re-shape");
        assert_eq!(stats.hits, 0, "the widget skipped the text context");
    }

    #[test]
    fn a_changed_span_list_requests_layout_and_reshapes() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([span("alpha "), span("beta")]);
        let next = rich_text([span("alpha "), span("gamma")]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        let flags = View::<()>::rebuild(&next, &view, &mut widget, &mut BuildCtx::new(&mut 0u64));
        assert!(flags.contains(ChangeFlags::LAYOUT));
        layout_widget(&mut widget, None, 300.0, &mut tcx);
        assert_eq!(tcx.shape_cache_stats().shapes, 2);
    }

    #[test]
    fn empty_spans_drop_and_equal_neighbours_merge() {
        let view = rich_text([
            span("one "),
            span(""),
            span("two ").color(red()),
            span("three").color(red()),
            span("").color(green()),
            span("four"),
        ]);
        {
            let _slot = crate::test_support::LocaleSlotGuard::hold();
            let mut tcx = TextContext::new();
            let (widget, _) = lay_out(&view, None, 300.0, &mut tcx);
            assert_eq!(group_styles(&widget).len(), 3);
        }
        let colors = glyph_colors(&painted(&view, None));
        assert_eq!(colors.len(), 3);
        assert_eq!(colors[1], red());
    }

    #[test]
    fn grouping_folds_the_excess_into_the_last_kept_group() {
        let styles: Vec<TextStyle> = (0..6)
            .map(|i| TextStyle::new(10.0 + (i % 2) as f32, Color::BLACK))
            .collect();
        let texts = ["a"; 6];
        let groups = group_spans(texts.iter().copied(), &styles, 4);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[3].style, 3);
        assert_eq!(
            groups[3].spans,
            3..6,
            "the excess joins the last kept group"
        );
        let none = group_spans(["", ""].into_iter(), &styles, 4);
        assert_eq!(none.len(), 1, "all-empty text keeps its first span");
    }

    #[test]
    fn more_spans_than_the_shaper_indexes_degrade_instead_of_panicking() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        // Alternating explicit sizes, so no neighbours merge.
        let count = usize::from(u16::MAX) + 4_000;
        let view =
            rich_text((0..count).map(|i| span("a").size(if i % 2 == 0 { 12.0 } else { 13.0 })));
        let mut tcx = TextContext::new();
        let (widget, size) = lay_out(&view, None, 300.0, &mut tcx);
        assert_eq!(group_styles(&widget).len(), usize::from(u16::MAX));
        assert!(size.width > 0.0);
    }

    /// Paint ops for the background and link tests: rounded fills, plain
    /// fills and every glyph's position.
    #[derive(Debug, PartialEq)]
    enum Mark {
        Round(Rect, f64, Color),
        Rect,
        Glyphs(Vec<(f32, f32)>),
    }

    #[derive(Default)]
    struct MarkRecorder(Vec<Mark>);

    impl PaintScene for MarkRecorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {
            self.0.push(Mark::Rect);
        }
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, c: Color) {
            self.0
                .push(Mark::Round(Rect::from_origin_size(o, s), radius, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.0.push(Mark::Glyphs(
                run.glyphs.iter().map(|g| (g.x, g.y)).collect(),
            ));
        }
    }

    fn marks(widget: &mut RichTextWidget) -> Vec<Mark> {
        let mut rec = MarkRecorder::default();
        widget.paint(
            &mut PaintCtx::new(Point::ZERO, Size::new(300.0, 400.0)),
            &mut rec,
        );
        rec.0
    }

    fn grey() -> Color {
        Color::from_rgb8(220, 220, 220)
    }

    /// A paragraph whose middle span wraps across lines at a 120 px width.
    fn wrapped(middle: SpanView) -> RichTextView {
        rich_text([span("see "), middle, span(" after")])
    }

    const CODE: &str = "alpha beta gamma delta epsilon";

    #[test]
    fn a_wrapped_background_paints_one_padded_rect_per_line_fragment() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let (mut widget, size) = lay_out(
            &wrapped(span(CODE).background(grey())),
            None,
            120.0,
            &mut tcx,
        );
        let fragments = widget.fragments.clone();
        assert!(fragments.len() >= 2, "the span wraps: {fragments:?}");
        let first = fragments[0].rect;
        assert_eq!(first.y0, 0.0, "it starts on the first line");
        assert!(first.x0 > 0.0, "it starts mid-line, after \"see \"");
        for pair in fragments.windows(2) {
            assert_eq!(pair[0].rect.y1, pair[1].rect.y0, "line boxes stack");
            assert!(pair[0].rect.height() > 0.0);
        }
        assert!(fragments.last().unwrap().rect.y1 <= size.height + 0.01);

        let ops = marks(&mut widget);
        let rounds: Vec<(Rect, f64, Color)> = ops
            .iter()
            .filter_map(|op| match op {
                Mark::Round(r, radius, c) => Some((*r, *radius, *c)),
                _ => None,
            })
            .collect();
        assert_eq!(rounds.len(), fragments.len());
        for ((rect, radius, color), fragment) in rounds.iter().zip(&fragments) {
            assert_eq!(*rect, fragment.rect.inflate(BG_PAD_X, 0.0));
            assert_eq!(*radius, BG_RADIUS);
            assert_eq!(*color, grey());
        }
        let first_glyphs = ops.iter().position(|op| matches!(op, Mark::Glyphs(_)));
        assert!(
            first_glyphs.is_some_and(|g| g >= rounds.len()),
            "backgrounds paint under the glyphs: {ops:?}"
        );
    }

    fn glyph_positions(ops: &[Mark]) -> Vec<(f32, f32)> {
        ops.iter()
            .flat_map(|op| match op {
                Mark::Glyphs(g) => g.clone(),
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn a_background_or_link_changes_neither_size_nor_glyph_positions() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let (mut plain, plain_size) = lay_out(&wrapped(span(CODE)), None, 120.0, &mut tcx);
        let reference = glyph_positions(&marks(&mut plain));
        for marked in [
            span(CODE).background(grey()),
            span(CODE).link("x"),
            span(CODE).themed_background(SurfaceRole::SurfaceContainerHigh),
        ] {
            let (mut widget, size) = lay_out(&wrapped(marked), None, 120.0, &mut tcx);
            assert_eq!(size, plain_size);
            assert_eq!(glyph_positions(&marks(&mut widget)), reference);
            assert_eq!(
                widget.layout.as_ref().map(TextLayout::line_count),
                plain.layout.as_ref().map(TextLayout::line_count)
            );
        }
    }

    #[test]
    fn a_background_resolves_explicit_then_themed_then_fallback() {
        let theme = Theme::neutral();
        let high = theme.scheme().surface_container_high;
        let themed = span("x").themed_background(SurfaceRole::SurfaceContainerHigh);
        assert_eq!(themed.background_color(Some(&theme)), Some(high));
        assert_eq!(themed.background_color(None), Some(BG_FALLBACK));
        let explicit_last = themed.clone().background(red());
        let explicit_first = span("x")
            .background(red())
            .themed_background(SurfaceRole::SurfaceContainerHigh);
        for s in [explicit_last, explicit_first] {
            assert_eq!(s.background_color(Some(&theme)), Some(red()));
        }
        assert_eq!(span("x").background_color(Some(&theme)), None);
    }

    #[test]
    fn a_link_hits_on_every_fragment_and_misses_elsewhere() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let (widget, size) = lay_out(&wrapped(span(CODE).link("docs")), None, 120.0, &mut tcx);
        assert_eq!(widget.links.len(), 1);
        assert!(widget.fragments.len() >= 2, "{:?}", widget.fragments);
        for fragment in &widget.fragments {
            assert_eq!(widget.link_at(fragment.rect.center()), Some(0));
        }
        let first = widget.fragments[0].rect;
        assert_eq!(widget.link_at(Point::new(1.0, first.center().y)), None);
        assert_eq!(
            widget.link_at(Point::new(first.x0 - BG_PAD_X / 2.0, 1.0)),
            None
        );
        assert_eq!(widget.link_at(Point::new(5.0, size.height + 5.0)), None);
    }

    #[test]
    fn link_spans_group_apart_from_equal_style_neighbours() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([
            span("a "),
            span("b").link("one"),
            span("c").link("one").weight(FontWeight::BOLD),
            span(" d "),
            span("e").link("two"),
        ]);
        let runs = link_runs(&view.spans);
        let ids: Vec<&str> = runs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["one", "two"]);
        assert_eq!(runs[0].label, "bc");
        assert_eq!(runs[0].spans, 1..3);

        let mut tcx = TextContext::new();
        let (widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        let spans: Vec<usize> = widget.fragments.iter().map(|f| f.span).collect();
        assert_eq!(spans, [1, 2, 4], "one fragment per marked group");
        assert_eq!(widget.link_of(2), Some(0));
        assert_eq!(widget.link_of(4), Some(1));
    }

    #[test]
    fn a_mid_line_span_in_right_to_left_text_gets_its_visual_extent() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let view = rich_text([
            span("\u{5d0}\u{5d1}\u{5d2} "),
            span("\u{5d3}\u{5d4}").background(grey()).link("he"),
            span(" \u{5d5}\u{5d6}\u{5d7}"),
        ]);
        let mut tcx = TextContext::new();
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        assert_eq!(widget.fragments.len(), 1, "{:?}", widget.fragments);
        let rect = widget.fragments[0].rect;
        // The line is right-aligned; the span sits between the other two.
        let xs: Vec<f64> = glyph_positions(&marks(&mut widget))
            .iter()
            .map(|&(x, _)| f64::from(x))
            .collect();
        let (left, right) = xs
            .iter()
            .fold((f64::MAX, f64::MIN), |(l, r), &x| (l.min(x), r.max(x)));
        assert!(
            rect.x0 > left && rect.x1 < right,
            "{rect:?} in {left}..{right}"
        );
        assert_eq!(widget.link_at(rect.center()), Some(0));
        assert_eq!(
            widget.link_at(Point::new(rect.x0 - 3.0, rect.center().y)),
            None
        );
        assert_eq!(
            widget.link_at(Point::new(rect.x1 + 3.0, rect.center().y)),
            None
        );
    }

    #[test]
    fn a_link_cut_by_max_lines_has_no_fragment() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let words = "word ".repeat(40);
        let view = rich_text([span(words), span("tail").link("gone")]).max_lines(1);
        let mut tcx = TextContext::new();
        let (widget, _) = lay_out(&view, None, 120.0, &mut tcx);
        assert!(widget.fragments.is_empty(), "{:?}", widget.fragments);
    }

    #[test]
    fn spans_and_views_compare_by_content_not_by_callback() {
        let make = |id: &str| {
            rich_text([span("x").background(grey()).link(id)]).on_link(|_: &mut (), _| {})
        };
        assert_eq!(make("a"), make("a"));
        assert_ne!(make("a"), make("b"));
        assert_ne!(
            make("a"),
            rich_text([span("x").background(grey()).link("a")]),
            "having a callback is part of the view"
        );
        assert_ne!(span("x").background(grey()), span("x"));
    }

    /// The shape cache's events (shapes, re-breaks and hits): one per
    /// layout of any cap, so its delta counts the layouts a call made.
    fn layout_events(tcx: &TextContext) -> u64 {
        let stats = tcx.shape_cache_stats();
        stats.shapes + stats.line_breaks + stats.hits
    }

    /// `lines` one-word lines at 40 px wide, with `marked` giving the line
    /// indices whose word is a link. Returns the widget, its height and the
    /// layouts the widget's first layout made.
    fn tall_paragraph(lines: usize, marked: &[usize]) -> (RichTextWidget, f64, u64) {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let spans: Vec<SpanView> = (0..lines)
            .map(|i| {
                let word = span(if i == 0 {
                    "word000".to_owned()
                } else {
                    format!("\nword{i:03}")
                });
                if marked.contains(&i) {
                    word.link("x")
                } else {
                    word
                }
            })
            .collect();
        let view = rich_text(spans);
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut 0u64));
        let before = layout_events(&tcx);
        let size = layout_widget(&mut widget, None, 300.0, &mut tcx);
        (widget, size.height, layout_events(&tcx) - before)
    }

    #[test]
    fn measuring_a_link_on_the_last_line_does_not_scale_with_the_lines_above() {
        let (small, _, small_layouts) = tall_paragraph(40, &[39]);
        let (large, _, large_layouts) = tall_paragraph(90, &[89]);
        assert_eq!(small.fragments.len(), 1);
        assert_eq!(large.fragments.len(), 1);
        // The fragment is the last of at least 40 line boxes.
        let f = small.fragments[0].rect;
        assert!(f.y1 / f.height() >= 39.5, "{f:?}");
        let f = large.fragments[0].rect;
        assert!(f.y1 / f.height() >= 89.5, "{f:?}");
        assert_eq!(
            small_layouts, large_layouts,
            "extra layouts must not grow with the unmarked lines above"
        );
        assert!(small_layouts <= 6, "{small_layouts} layouts");
    }

    #[test]
    fn measuring_marks_on_two_adjacent_lines_is_bounded() {
        let (small, _, small_layouts) = tall_paragraph(40, &[20, 21]);
        let (large, _, large_layouts) = tall_paragraph(90, &[60, 61]);
        assert_eq!(small.fragments.len(), 2);
        assert_eq!(large.fragments.len(), 2);
        let (a, b) = (small.fragments[0].rect, small.fragments[1].rect);
        assert!(
            (a.y1 - b.y0).abs() < 0.01,
            "adjacent line boxes share an edge"
        );
        assert!(a.y0 > 0.0 && b.y1 > b.y0);
        assert_eq!(small_layouts, large_layouts);
        assert!(small_layouts <= 7, "{small_layouts} layouts");
    }

    #[test]
    fn a_default_block_builds_no_selection_machinery() {
        let plain = rich_text([span("text")]);
        let selectable = plain.clone().selectable(true);
        assert_ne!(plain, selectable, "selectable is part of the view");
        let widget = View::<()>::build(&plain, &mut BuildCtx::new(&mut 0u64));
        assert!(widget.select.is_none());
        assert!(!widget.is_focusable());
        let widget = View::<()>::build(&selectable, &mut BuildCtx::new(&mut 0u64));
        assert!(widget.select.is_some());
        assert!(!widget.is_focusable(), "no focus stop at rest");
    }

    #[test]
    fn turning_selectable_on_and_off_builds_and_drops_the_machinery() {
        let plain = rich_text([span("text")]);
        let selectable = plain.clone().selectable(true);
        let mut ctx_id = 0u64;
        let mut widget = View::<()>::build(&plain, &mut BuildCtx::new(&mut ctx_id));
        let flags = View::<()>::rebuild(
            &selectable,
            &plain,
            &mut widget,
            &mut BuildCtx::new(&mut ctx_id),
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(widget.select.is_some());
        let flags = View::<()>::rebuild(
            &plain,
            &selectable,
            &mut widget,
            &mut BuildCtx::new(&mut ctx_id),
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
        assert!(widget.select.is_none());
        let again = plain.clone();
        let flags =
            View::<()>::rebuild(&again, &plain, &mut widget, &mut BuildCtx::new(&mut ctx_id));
        assert_eq!(flags, ChangeFlags::NONE, "equal views still short-circuit");
    }

    /// A selectable block over `text`, laid out at 300 px, with `range`
    /// selected.
    fn selected(text: &str, range: Range<usize>, tcx: &mut TextContext) -> RichTextWidget {
        let (mut widget, _) = lay_out(&rich_text([span(text)]).selectable(true), None, 300.0, tcx);
        if let Some(select) = widget.select.as_mut() {
            select.range = range;
            select.focused = true;
        }
        widget
    }

    #[test]
    fn a_selection_survives_a_relayout_and_drops_with_its_text() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let view = rich_text([span("alpha beta")]).selectable(true);
        let mut widget = selected("alpha beta", 0..5, &mut tcx);
        let mut ctx_id = 0u64;
        let wider = view.clone().align(TextAlign::Center);
        View::<()>::rebuild(&wider, &view, &mut widget, &mut BuildCtx::new(&mut ctx_id));
        layout_widget(&mut widget, None, 200.0, &mut tcx);
        assert_eq!(widget.select.as_ref().map(|s| s.range.clone()), Some(0..5));
        let other = rich_text([span("gamma delta")]).selectable(true);
        View::<()>::rebuild(&other, &wider, &mut widget, &mut BuildCtx::new(&mut ctx_id));
        assert_eq!(widget.select.as_ref().map(|s| s.range.clone()), Some(0..0));
    }

    #[test]
    fn the_toolbar_offers_copy_and_select_all_only() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let part = selected("alpha beta", 0..5, &mut tcx);
        assert_eq!(
            part.toolbar_actions(),
            SelectionToolbarActions {
                copy: true,
                cut: false,
                paste: false,
                select_all: true,
            }
        );
        let all = selected("alpha beta", 0..10, &mut tcx);
        assert!(all.toolbar_actions().copy && !all.toolbar_actions().select_all);
        let none = selected("alpha beta", 0..0, &mut tcx);
        assert!(!none.toolbar_actions().copy && none.toolbar_actions().select_all);
    }

    #[test]
    fn a_held_word_extends_from_its_far_end() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let text = "alpha beta gamma";
        let widget = selected(text, 6..10, &mut tcx);
        let layout = widget.layout.as_ref().expect("laid out");
        let point = |i: usize| {
            let caret = layout.caret_rect(TextPosition::new(i, TextAffinity::Downstream), 0.0);
            Point::new(caret.x0 + 0.5, caret.center().y)
        };
        let word = 6..10;
        assert_eq!(widget.extend_word(&word, point(13)), 6..13);
        assert_eq!(widget.extend_word(&word, point(2)), 2..10);
        assert_eq!(widget.extend_word(&word, point(8)), 6..10);
    }

    #[test]
    fn a_handle_is_grabbed_at_either_end_and_keeps_the_other() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let mut widget = selected("alpha beta gamma", 6..10, &mut tcx);
        let [start, end] = widget.end_carets(&(6..10)).expect("laid out");
        let select = widget.select.as_deref().expect("selectable");
        assert_eq!(
            widget.handle_at(select, start.center()),
            None,
            "no handles without a touch selection"
        );
        if let Some(select) = widget.select.as_mut() {
            select.handles = true;
        }
        let select = widget.select.as_deref().expect("selectable");
        let (fixed, _) = widget.handle_at(select, start.center()).expect("start");
        assert_eq!(fixed, 10, "dragging the start keeps the end");
        let (fixed, grab) = widget.handle_at(select, end.center()).expect("end");
        assert_eq!(fixed, 6, "dragging the end keeps the start");
        assert!((end.center() + grab - Point::new(end.x0, end.center().y)).hypot() < 1e-9);
        assert_eq!(widget.handle_at(select, Point::new(1.0, 1.0)), None);
    }

    #[test]
    fn marked_fragments_skip_a_stale_or_off_boundary_range() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let text = "h\u{e9}llo";
        let layout = tcx.layout(text, &TextStyle::default(), Some(300.0));
        // Past the end, and inside the two-byte e-acute (bytes 1..3).
        for range in [0..100, 2..4, 6..9] {
            let marked = [(range, 0)];
            let _ = marked_fragments(&layout, text, &marked);
        }
        let whole = marked_fragments(&layout, text, &[(0..text.len(), 0)]);
        assert!(!whole.is_empty(), "a valid range still yields fragments");
    }

    #[test]
    fn replacing_the_text_ends_the_press_and_lowers_its_vetoes() {
        let _slot = crate::test_support::LocaleSlotGuard::hold();
        let mut tcx = TextContext::new();
        let view = rich_text([span("alpha beta")]).selectable(true);
        let (mut widget, _) = lay_out(&view, None, 300.0, &mut tcx);
        let veto = Rc::new(Cell::new(false));
        let select = widget.select.as_deref_mut().expect("selectable");
        select.vetoes = vec![Rc::clone(&veto)];
        select.raise_vetoes();
        select.press = SelectPress::Drag { anchor: 7 };
        select.range = 3..7;
        select.focused = true;
        assert!(veto.get(), "the press holds the scroll lock");

        let next = rich_text([span("ab")]).selectable(true);
        let mut id = 0u64;
        View::<()>::rebuild(&next, &view, &mut widget, &mut BuildCtx::new(&mut id));
        let select = widget.select.as_deref().expect("still selectable");
        assert!(!veto.get(), "the lock is released");
        assert_eq!(select.press, SelectPress::None);
        assert!(select.range.is_empty());
        assert!(!select.focused, "focus nothing needs is given back");
    }
}
