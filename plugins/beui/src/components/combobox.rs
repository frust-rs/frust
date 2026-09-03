//! Ports beUI's **Combobox** — the search field whose panel morphs open under
//! it and whose list narrows live as the query is typed.
//!
//! Source: `components/motion/combobox/{context,trigger,content,list}.tsx` and
//! `use-active-option.ts` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `combobox`: *"Searchable combobox with a morphing portal, grouped
//! filtering, keyboard navigation, and controlled or uncontrolled state."*
//!
//! | class / prop | here |
//! |---|---|
//! | trigger `h-10 rounded-xl border border-border px-3 cursor-text` | [`crate::components::input`]'s field chrome (see the height note below) |
//! | trigger `hover:border-(--color-border-strong)`, `focus-within:ring-2` | the wrapped field's own hover/focus resolution |
//! | `ChevronsUpDown` `size-4` | [`select::CHEVRON_BOX`], drawn as the paired chevrons |
//! | content `rounded-xl border border-border bg-background`, `minWidth: triggerWidth` | [`select::PANEL_RADIUS`], the anchor's width as a floor |
//! | content `sideOffset = 6` | [`PANEL_OFFSET`] |
//! | content height/opacity `spring duration 0.5 bounce 0.22` | the row presence lanes on [`SPRING_LAYOUT`] |
//! | list `max-h-64 overflow-y-auto p-1.5` | [`COMBOBOX_MAX_HEIGHT`], [`LIST_PADDING`] |
//! | item `rounded-lg px-2 py-2 text-sm`, active `bg-muted` | [`select::ITEM_RADIUS`], [`ITEM_PADDING_X`], the hover wash |
//! | item check `opacity/scale 0.82 → 1`, `duration 0.14` | [`select::CHECK_SIZE`], the check drawn at rest |
//! | empty `px-3 py-8 text-center text-sm` | [`EMPTY_PADDING_Y`] |
//! | `defaultFilter` — case-folded subsequence over value + keywords | [`combobox_matches`] |
//!
//! # Shape of the port
//!
//! Upstream is a context plus four component files; a frust app already owns its
//! state, so the port is the two halves it mounts — mirroring
//! [`crate::components::select`]:
//!
//! * [`combobox_trigger`] — the field, capturing its own window rect into an
//!   [`OverlayAnchor`], reporting each edit as a query change and each press as
//!   an open request.
//! * [`combobox`] — the filtered panel, anchored to that rect through
//!   [`crate::overlay::anchored`](mod@crate::overlay::anchored), reporting a commit as an option index.
//!
//! # The editing is `input`'s, not this module's
//!
//! The trigger wraps [`crate::components::input`]'s controlled field rather than
//! re-deriving one: the text editor, the caret, focus publication, the field's
//! border/ring crossfade and its disabled treatment are all already that
//! component's, and a second editable here would be a second set of them to keep
//! in sync. This module adds the chevrons, the anchor capture and the open
//! request on top.
//!
//! # Filtering, and what "live" means
//!
//! [`combobox_matches`] is upstream's `defaultFilter` exactly: case-folded, and
//! a **subsequence** rather than a substring match, so `bnn` finds *Banana*.
//! An empty (or whitespace-only) query matches everything. A row that leaves the
//! filtered set collapses its own height and fades out where it stands, and a
//! row that joins it grows back in — the presence-animated add/remove upstream
//! gets from its layout animation. That makes the panel's own height an
//! animated quantity, so the paint arm asks for a relayout while any row is
//! moving (the one-frame lag `crate::motion` describes).
//!
//! # v1 boundaries, adopted from the sibling catalog
//!
//! * **No type-ahead, and no keyboard list navigation.** Upstream's
//!   `use-active-option.ts` moves an active row with Arrow/Home/End and commits
//!   it with Enter, all from keys the `<input>` sees. Here the wrapped field
//!   owns the focus path and the framework offers no seam to forward its
//!   declined keys to a sibling overlay, so the pointer is the only way to move
//!   the highlight and commit a row. Escape still dismisses, through the
//!   overlay seam.
//! * **The height cap is a constant** ([`COMBOBOX_MAX_HEIGHT`], upstream's
//!   `max-h-64`), and a longer list clips rather than scrolls — a scrollable
//!   panel would have to mount inside a [`frust::scroll_view`], which the
//!   overlay seam forbids under a host.
//!
//! # Degradations
//!
//! - **No groups, labels or separators.** Upstream ships `ComboboxGroup` /
//!   `ComboboxLabel` / `ComboboxSeparator` and hides a group whose rows all
//!   filtered out. The port is a flat list; a grouped combobox is additive.
//! - **The trigger is `h-11`, not `h-10`.** It is the catalog's own field
//!   ([`style::HEIGHT_INPUT`]), which is the rung `input` ports and the one a
//!   combobox beside a beUI text field has to match.
//! - **The chevrons sit in the field's trailing padding.** The wrapped field
//!   takes one symmetric horizontal padding, so the affordance overlays that
//!   padding rather than reserving space of its own — the same call `input`
//!   documents for its success check.
//! - **No active-row highlight travel.** Upstream slides one `layoutId` pill
//!   between rows; the hover wash is painted per row here, so it cuts rather
//!   than travels.

use std::cell::RefCell;
use std::rc::Rc;

use frust::Theme;
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedArgCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect, Role,
    SemanticsCtx, Size, View, Widget, any, build_child, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child, visit_children,
};

use super::input::input;
use super::select::{self, PANEL_RADIUS, draw_check, draw_chevron, panel_colors, stroke_frame};
use crate::motion::Ramp;
use crate::overlay::anchored::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::press::{Lane, inside_inclusive, presses};
use crate::style;
use crate::text::{LabelRun, label_style};
use crate::tokens::motion::SPRING_LAYOUT;

// ---- Metrics ---------------------------------------------------------------

/// The trigger's height, in logical px — the catalog's field rung (see the
/// [module docs](self)' height note).
pub const TRIGGER_HEIGHT: f64 = style::HEIGHT_INPUT;

/// The gap between the trigger and the panel, in logical px
/// (`sideOffset = 6`).
pub const PANEL_OFFSET: f64 = 6.0;

/// Padding inside the panel, around the list, in logical px (`p-1.5`).
pub const LIST_PADDING: f64 = 6.0;

/// Horizontal padding inside a row, in logical px (`px-2`).
pub const ITEM_PADDING_X: f64 = 8.0;

/// Vertical padding inside a row, in logical px (`py-2`).
pub const ITEM_PADDING_Y: f64 = 8.0;

/// A row's full height, in logical px — `py-2` around a `text-sm` line.
pub const ITEM_HEIGHT: f64 = style::TEXT_SM * 1.5 + ITEM_PADDING_Y * 2.0;

/// Vertical padding around the empty-state row, in logical px (`py-8`).
pub const EMPTY_PADDING_Y: f64 = 32.0;

/// The empty-state row's full height, in logical px.
pub const EMPTY_HEIGHT: f64 = style::TEXT_SM * 1.5 + EMPTY_PADDING_Y * 2.0;

/// The list viewport's cap, in logical px (`max-h-64`).
pub const COMBOBOX_MAX_HEIGHT: f64 = 256.0;

/// The text an empty result set shows (`"No options found."`).
pub const EMPTY_LABEL: &str = "No options found.";

/// Opacity of a disabled row: `disabled:opacity-45`.
const ROW_DISABLED_OPACITY: f32 = 0.45;

// ---- Options and filtering -------------------------------------------------

/// One option in a [`combobox`] list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComboboxOption {
    label: String,
    keywords: Vec<String>,
    disabled: bool,
}

/// Create an option labelled `label`.
pub fn combobox_option(label: impl Into<String>) -> ComboboxOption {
    ComboboxOption {
        label: label.into(),
        keywords: Vec::new(),
        disabled: false,
    }
}

impl ComboboxOption {
    /// Add search terms the option also matches on, beyond its own label —
    /// upstream's `keywords` prop.
    pub fn keywords<I: IntoIterator<Item = S>, S: Into<String>>(mut self, keywords: I) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// Make the option unselectable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The option's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The option's extra search terms.
    pub fn search_terms(&self) -> &[String] {
        &self.keywords
    }

    /// Whether the option is unselectable.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Whether `query` matches an option labelled `label` with these `keywords` —
/// upstream's `defaultFilter`, exactly.
///
/// The needle is trimmed and lowercased; an empty one matches everything. The
/// haystack is the label followed by the keywords, joined by spaces and
/// lowercased, and the match is a **subsequence** rather than a substring: every
/// character of the needle has to appear in order, not adjacently. That is what
/// makes `bnn` find *Banana* and `usa` find *United States of America*.
///
/// Case folding is `to_lowercase`, which is Unicode-aware — the closest
/// available reading of JavaScript's `toLocaleLowerCase()` without a locale.
pub fn combobox_matches(label: &str, keywords: &[String], query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let mut haystack = String::from(label);
    for keyword in keywords {
        haystack.push(' ');
        haystack.push_str(keyword);
    }
    let haystack = haystack.to_lowercase();
    let mut wanted = needle.chars();
    let mut next = wanted.next();
    for character in haystack.chars() {
        let Some(target) = next else {
            return true;
        };
        if character == target {
            next = wanted.next();
        }
    }
    next.is_none()
}

// ---- The trigger -----------------------------------------------------------

/// A view-held, typed text callback (erased by the wrapped field).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held, typed open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI combobox trigger. See [`combobox_trigger`].
pub struct ComboboxTriggerView<State: 'static> {
    anchor: OverlayAnchor,
    query: String,
    selected_label: Option<String>,
    placeholder: String,
    open: bool,
    disabled: bool,
    on_query_change: OnText<State>,
    on_open_change: OnOpenChange<State>,
}

/// Create the combobox's trigger: the search field showing `query` while open
/// (and the selected label while closed), capturing its own window rect into
/// `anchor` and reporting each edit through `on_query_change`.
pub fn combobox_trigger<State: 'static, F: Fn(&mut State, String) + 'static>(
    anchor: &OverlayAnchor,
    query: impl Into<String>,
    on_query_change: F,
) -> ComboboxTriggerView<State> {
    ComboboxTriggerView {
        anchor: anchor.clone(),
        query: query.into(),
        selected_label: None,
        placeholder: String::from("Search…"),
        open: false,
        disabled: false,
        on_query_change: Rc::new(on_query_change),
        on_open_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> ComboboxTriggerView<State> {
    /// Show `label` while the panel is closed — upstream's
    /// `value={open ? query : selectedLabel ?? ""}`.
    pub fn selected_label(mut self, label: Option<String>) -> Self {
        self.selected_label = label;
        self
    }

    /// Set the field's placeholder (`"Search…"`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Tell the trigger whether the panel is open — which text it shows, and
    /// whether a press asks to open.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Disable the control: the wrapped field's own disabled treatment, and no
    /// open requests.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the open-request callback: a press inside the field reports `true`.
    ///
    /// Only ever `true` — a *close* comes from the overlay seam's dismissal, not
    /// from here, so the two never race to report the same gesture.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }

    /// The text the wrapped field is controlled with.
    fn field_text(&self) -> String {
        if self.open {
            self.query.clone()
        } else {
            self.selected_label.clone().unwrap_or_default()
        }
    }
}

/// The retained widget for a [`ComboboxTriggerView`].
pub struct ComboboxTriggerWidget {
    /// The wrapped beUI field — the whole of the editing surface.
    field: ChildPod,
    anchor: OverlayAnchor,
    open: bool,
    disabled: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for ComboboxTriggerView<State> {
    type Element = ComboboxTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ComboboxTriggerWidget {
        ComboboxTriggerWidget {
            field: build_child(&self.field_view(), ctx),
            anchor: self.anchor.clone(),
            open: self.open,
            disabled: self.disabled,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ComboboxTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        element.anchor = self.anchor.clone();
        element.open = self.open;
        element.disabled = self.disabled;
        rebuild_child(
            &prev.field_view(),
            &self.field_view(),
            &mut element.field,
            ctx,
        )
    }

    fn teardown(&self, element: &mut ComboboxTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.field_view(), &mut element.field, ctx);
    }
}

impl<State: 'static> ComboboxTriggerView<State> {
    /// The wrapped field, rebuilt from this view's own props.
    ///
    /// Built on demand rather than stored so the view stays a plain data
    /// description: the field is `input`'s, and this is the one place its props
    /// are assembled.
    fn field_view(&self) -> AnyView<State> {
        let on_change = self.on_query_change.clone();
        any(
            input::<State, _>(self.field_text(), move |state: &mut State, text| {
                on_change(state, text);
            })
            .placeholder(self.placeholder.clone())
            .disabled(self.disabled),
        )
    }
}

impl Widget for ComboboxTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.field.layout_child(ctx, bc);
        self.field.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let colors = panel_colors(theme);
        let origin = ctx.origin();
        let size = ctx.size();

        // The anchored panel is placed against this rect — the same capture
        // `overlay::anchor` performs, done here directly because this widget is
        // already the trigger.
        self.anchor.set(Rect::from_origin_size(origin, size));

        self.field.paint_child(ctx, scene);

        // `ChevronsUpDown`: two chevrons back to back, in the field's trailing
        // padding (see the module docs' padding note).
        let centre = Point::new(
            origin.x + size.width - style::PADDING_X_INPUT - select::CHEVRON_BOX / 2.0,
            origin.y + size.height / 2.0,
        );
        let ink = style::disabled_tint(colors.muted, self.disabled, style::DISABLED_OPACITY_INPUT);
        let half = select::CHEVRON_BOX / 4.0;
        draw_chevron(
            scene,
            Point::new(centre.x, centre.y - half),
            select::CHEVRON_SPIN,
            ink,
            select::CHEVRON_BOX,
        );
        draw_chevron(
            scene,
            Point::new(centre.x, centre.y + half),
            0.0,
            ink,
            select::CHEVRON_BOX,
        );
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The field first, always — it owns the caret, the focus session and the
        // editing (the container-claims-after-routing rule).
        let routed = route_event_single(&mut self.field, ctx, event);
        if self.disabled || self.open {
            return routed;
        }
        // ...then the open request, which upstream fires from the trigger
        // wrapper's own `onPointerDown` regardless of what the input did with
        // the press. A press is what opens the panel; the *close* is the
        // overlay seam's dismissal, never this arm.
        if let InputEvent::Pointer(p) = event
            && p.phase == PointerPhase::Down
            && presses(p)
            && inside_inclusive(p.position, ctx.size())
        {
            (self.on_open_change)(ctx, true);
            return EventResult::Handled;
        }
        routed
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // The wrapped field publishes the editable node; this wrapper adds the
        // combobox framing around it.
        ctx.push_container(
            Role::ComboBox,
            |node| {
                node.set_expanded(self.open);
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| self.field.semantics_child(ctx),
        );
    }

    visit_children!(field);
}

// ---- The panel -------------------------------------------------------------

/// What the panel needs from its builder chain after the content view has
/// already been wrapped in the host.
#[derive(Default)]
struct PanelConfig {
    anchor: Option<OverlayAnchor>,
    open: bool,
}

/// A shared handle onto the panel's late-bound configuration.
type PanelHandle = Rc<RefCell<PanelConfig>>;

/// A declarative beUI combobox panel. See [`combobox`].
pub struct ComboboxView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    config: PanelHandle,
    placement: OverlayPlacement,
}

/// A view-held, typed commit callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// Build a combobox panel over `options`, filtered live by `query`, showing
/// `options[selected]` as checked and reporting a commit through
/// `on_select(state, option_index)`.
///
/// `option_index` indexes the **original** `options`, not the filtered view, so
/// a caller never has to map a row back itself.
pub fn combobox<State: 'static, F: Fn(&mut State, usize) + 'static>(
    options: Vec<ComboboxOption>,
    selected: Option<usize>,
    query: impl Into<String>,
    on_select: F,
) -> ComboboxView<State> {
    let config: PanelHandle = Rc::new(RefCell::new(PanelConfig {
        open: true,
        ..PanelConfig::default()
    }));
    let content = ComboboxPanelView {
        options,
        selected,
        query: query.into(),
        config: config.clone(),
        on_select: Rc::new(on_select),
    };
    let placement = OverlayPlacement::default()
        .align(OverlayAlign::Start)
        .offset(PANEL_OFFSET);
    ComboboxView {
        inner: anchored(content).placement(placement),
        config,
        placement,
    }
}

impl<State: 'static> ComboboxView<State> {
    /// Anchor the panel to the trigger's captured rect — which is also where its
    /// minimum width comes from (`minWidth: triggerWidth`).
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.config.borrow_mut().anchor = Some(anchor.clone());
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the panel opens on (default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Set the cross-axis alignment (default `start`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.inner = self.inner.placement(self.placement);
        self
    }

    /// Hand a **kept-mounted** panel the app's open flag, so closing it plays
    /// the exit instead of vanishing. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: an outside press or a focus-routed Escape
    /// reports `false`, through the overlay seam.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }
}

impl<State: 'static> View<State> for ComboboxView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

/// The panel's content view. Never mounted on its own; [`combobox`] wraps it.
struct ComboboxPanelView<State: 'static> {
    options: Vec<ComboboxOption>,
    selected: Option<usize>,
    query: String,
    config: PanelHandle,
    on_select: OnSelect<State>,
}

/// One filtered row's retained state.
struct FilterRow {
    text: LabelRun,
    disabled: bool,
    /// How present the row is, `0.0` filtered out .. `1.0` in the set. Drives
    /// both its height and its alpha, which is what makes a filter change read
    /// as a collapse rather than a jump.
    presence: Lane,
}

/// The retained widget for the combobox panel.
pub struct ComboboxPanelWidget {
    rows: Vec<FilterRow>,
    selected: Option<usize>,
    config: PanelHandle,
    /// The empty-state row's own presence.
    empty: LabelRun,
    empty_presence: Lane,
    /// The panel's own measured size.
    content: Size,
    hovered: Option<usize>,
    captured: Option<usize>,
    on_select: ErasedArgCallback<usize>,
}

impl ComboboxPanelWidget {
    /// Row `index`'s current height, in logical px.
    fn row_height(&self, index: usize) -> f64 {
        self.rows.get(index).map_or(0.0, |row| {
            ITEM_HEIGHT * row.presence.value().clamp(0.0, 1.0)
        })
    }

    /// Row `index`'s box in the panel's own space, if it has any extent.
    fn row_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.rows.len() {
            return None;
        }
        let height = self.row_height(index);
        if height <= 0.0 {
            return None;
        }
        let top: f64 = (0..index).map(|i| self.row_height(i)).sum();
        Some(Rect::from_origin_size(
            Point::new(LIST_PADDING, LIST_PADDING + top),
            Size::new((self.content.width - LIST_PADDING * 2.0).max(0.0), height),
        ))
    }

    /// The row under a panel-local `pos` — only a row that is *staying* in the
    /// filtered set, so a half-collapsed one on its way out cannot be committed.
    fn hit_row(&self, pos: Point) -> Option<usize> {
        (0..self.rows.len()).find(|index| {
            let row = &self.rows[*index];
            !row.disabled
                && row.presence.target() >= 1.0
                && self.row_rect(*index).is_some_and(|r| r.contains(pos))
        })
    }

    /// The list's own height, in logical px, before the panel's cap.
    fn list_height(&self) -> f64 {
        let rows: f64 = (0..self.rows.len()).map(|i| self.row_height(i)).sum();
        rows + EMPTY_HEIGHT * self.empty_presence.value().clamp(0.0, 1.0)
    }

    /// The empty row's box in the panel's own space, if it has any extent.
    fn empty_rect(&self) -> Option<Rect> {
        let height = EMPTY_HEIGHT * self.empty_presence.value().clamp(0.0, 1.0);
        if height <= 0.0 {
            return None;
        }
        let top: f64 = (0..self.rows.len()).map(|i| self.row_height(i)).sum();
        Some(Rect::from_origin_size(
            Point::new(LIST_PADDING, LIST_PADDING + top),
            Size::new((self.content.width - LIST_PADDING * 2.0).max(0.0), height),
        ))
    }
}

/// Retarget every row's presence against `query`, reporting whether anything
/// moved. Shared by `build` (which lands the lanes at rest) and `rebuild`.
fn apply_filter(rows: &mut [FilterRow], options: &[ComboboxOption], query: &str) -> bool {
    let mut changed = false;
    for (row, option) in rows.iter_mut().zip(options) {
        let target = f64::from(u8::from(combobox_matches(
            &option.label,
            &option.keywords,
            query,
        )));
        if row.presence.target() != target {
            row.presence.retarget(target);
            changed = true;
        }
    }
    changed
}

/// How many of `options` `query` keeps.
fn visible_count(options: &[ComboboxOption], query: &str) -> usize {
    options
        .iter()
        .filter(|option| combobox_matches(&option.label, &option.keywords, query))
        .count()
}

/// The retained rows for `options`, each resting at its own visibility under
/// `query`.
fn rows_for(options: &[ComboboxOption], query: &str) -> Vec<FilterRow> {
    options
        .iter()
        .map(|option| FilterRow {
            text: LabelRun::new(option.label.clone()),
            disabled: option.disabled,
            presence: Lane::at_rest(
                Ramp::spring(SPRING_LAYOUT),
                f64::from(u8::from(combobox_matches(
                    &option.label,
                    &option.keywords,
                    query,
                ))),
            ),
        })
        .collect()
}

impl<State: 'static> View<State> for ComboboxPanelView<State> {
    type Element = ComboboxPanelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ComboboxPanelWidget {
        let empty = visible_count(&self.options, &self.query) == 0;
        ComboboxPanelWidget {
            rows: rows_for(&self.options, &self.query),
            selected: self.selected,
            config: self.config.clone(),
            empty: LabelRun::new(EMPTY_LABEL),
            empty_presence: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), f64::from(u8::from(empty))),
            content: Size::ZERO,
            hovered: None,
            captured: None,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ComboboxPanelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = erase_callback_arg(&self.on_select);
        element.config = self.config.clone();
        let mut flags = ChangeFlags::NONE;
        if prev.options != self.options {
            element.rows = rows_for(&self.options, &self.query);
            element.hovered = None;
            element.captured = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if prev.query != self.query
            && apply_filter(&mut element.rows, &self.options, &self.query)
        {
            // A row leaving the set keeps its hover until the pointer moves
            // again, which would paint a wash on a collapsing row.
            element.hovered = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let empty = f64::from(u8::from(visible_count(&self.options, &self.query) == 0));
        if element.empty_presence.target() != empty {
            element.empty_presence.retarget(empty);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.selected != self.selected {
            element.selected = self.selected;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for ComboboxPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(style::TEXT_SM);
        let mut widest: f64 = 0.0;
        for row in &mut self.rows {
            widest = widest.max(row.text.layout(ctx, &style).width);
        }
        widest = widest.max(self.empty.layout(ctx, &style).width);

        let anchor_width = self
            .config
            .borrow()
            .anchor
            .as_ref()
            .map_or(0.0, |anchor| anchor.rect().width());
        let natural = widest
            + (ITEM_PADDING_X + LIST_PADDING) * 2.0
            + select::TRIGGER_GAP
            + select::CHECK_SIZE
            + style::BORDER_WIDTH * 2.0;
        let width = natural.max(anchor_width).min(bc.max().width);
        let height = (LIST_PADDING * 2.0 + self.list_height())
            .min(COMBOBOX_MAX_HEIGHT)
            .min(bc.max().height);
        self.content = Size::new(width, height);
        bc.constrain(self.content)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = panel_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        let mut running = false;
        if reduce {
            for row in &mut self.rows {
                row.presence.snap();
            }
            self.empty_presence.snap();
        } else {
            for row in &mut self.rows {
                running |= row.presence.advance(now);
            }
            running |= self.empty_presence.advance(now);
        }

        let radius = style::resolve_radius(PANEL_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, colors.surface);
        stroke_frame(scene, origin, size, radius, colors.border);
        scene.push_clip_rounded(origin, size, radius);

        for index in 0..self.rows.len() {
            let Some(rect) = self.row_rect(index) else {
                continue;
            };
            let rect = rect + origin.to_vec2();
            let presence = self.rows[index].presence.value().clamp(0.0, 1.0);
            let layered = presence < 1.0;
            if layered {
                scene.push_layer(rect.origin(), rect.size(), presence as f32);
            }

            let selected = self.selected == Some(index);
            let hovered = self.hovered == Some(index);
            let disabled = self.rows[index].disabled;
            if hovered && !disabled {
                scene.fill_rounded_rect(
                    rect.origin(),
                    rect.size(),
                    select::ITEM_RADIUS,
                    colors.wash,
                );
            }
            let ink = style::disabled_tint(
                if hovered || selected {
                    colors.ink
                } else {
                    colors.muted
                },
                disabled,
                ROW_DISABLED_OPACITY,
            );
            let text = self.rows[index].text.size();
            self.rows[index].text.paint(
                Point::new(
                    rect.x0 + ITEM_PADDING_X,
                    rect.y0 + (rect.height() - text.height) / 2.0,
                ),
                ink,
                scene,
            );
            if selected {
                draw_check(
                    scene,
                    Point::new(
                        rect.x1 - ITEM_PADDING_X - select::CHECK_SIZE,
                        rect.y0 + (rect.height() - select::CHECK_SIZE) / 2.0,
                    ),
                    select::CHECK_SIZE,
                    ink,
                );
            }

            if layered {
                scene.pop_layer();
            }
        }

        if let Some(rect) = self.empty_rect() {
            let rect = rect + origin.to_vec2();
            let presence = self.empty_presence.value().clamp(0.0, 1.0);
            let layered = presence < 1.0;
            if layered {
                scene.push_layer(rect.origin(), rect.size(), presence as f32);
            }
            let text = self.empty.size();
            self.empty.paint(
                Point::new(
                    rect.x0 + (rect.width() - text.width) / 2.0,
                    rect.y0 + (rect.height() - text.height) / 2.0,
                ),
                colors.muted,
                scene,
            );
            if layered {
                scene.pop_layer();
            }
        }

        scene.pop_clip();

        // The row heights *are* the panel's height, so a bare frame request
        // would let a filtering list freeze on the intra-frame layout skip.
        if running {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.hit_row(p.position) else {
                    // Not a row: the host swallows the press for the panel.
                    return EventResult::Ignored;
                };
                self.captured = Some(index);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.captured.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                let hovered = self.hit_row(p.position);
                if hovered.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != hovered {
                    self.hovered = hovered;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let Some(index) = self.captured.take() else {
                    return EventResult::Ignored;
                };
                if self.hit_row(p.position) == Some(index) {
                    (self.on_select)(ctx, index);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured.take().is_none() {
                    return EventResult::Ignored;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let selected = self.selected;
        ctx.push_container(
            Role::ListBox,
            |_| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    // A filtered-out row is not merely invisible — it is not in
                    // the list at all, which is `if (!visible) return null`.
                    if row.presence.target() < 1.0 {
                        continue;
                    }
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(row.text.content());
                        node.set_selected(selected == Some(index));
                        if row.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::FrameTime;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, Brush, Color, PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    /// Records only stroke widths — the narrowest read that tells a check apart
    /// from the panel's hairline.
    struct CheckRecorder<'a> {
        widths: &'a mut Vec<f64>,
    }

    impl PaintScene for CheckRecorder<'_> {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, width: f64, _b: &Brush) {
            self.widths.push(width);
        }
    }

    #[derive(Clone, Default)]
    struct App {
        query: String,
        open: bool,
        picked: Vec<usize>,
        opens: Vec<bool>,
    }

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn options() -> Vec<ComboboxOption> {
        vec![
            combobox_option("Banana"),
            combobox_option("Blueberry").keywords(["berry", "blue"]),
            combobox_option("Cherry").disabled(true),
        ]
    }

    fn build<S: 'static, V: View<S>>(view: &V) -> V::Element {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout<W: Widget>(w: &mut W, bc: &BoxConstraints) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, bc)
    }

    fn paint_at<W: Widget>(w: &mut W, size: Size, ms: f64) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<W: Widget>(
        w: &mut W,
        state: &mut App,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    fn panel_view(query: &str, selected: Option<usize>) -> ComboboxPanelView<App> {
        ComboboxPanelView {
            options: options(),
            selected,
            query: query.to_string(),
            config: Rc::new(RefCell::new(PanelConfig {
                open: true,
                anchor: None,
            })),
            on_select: Rc::new(|s: &mut App, i| s.picked.push(i)),
        }
    }

    /// Rebuild a panel widget with a new query, the way a rebuild pass would.
    fn requery(w: &mut ComboboxPanelWidget, from: &str, to: &str) {
        let prev = panel_view(from, None);
        let next = panel_view(to, None);
        let mut counter = 0u64;
        View::<App>::rebuild(&next, &prev, w, &mut BuildCtx::new(&mut counter));
    }

    // ---- The filter -------------------------------------------------------

    /// `defaultFilter`'s own semantics: case-folded, subsequence, keyword-aware,
    /// and an empty needle keeps everything.
    #[test]
    fn the_filter_is_a_case_folded_subsequence_over_the_label_and_its_keywords() {
        let none: [String; 0] = [];
        assert!(combobox_matches("Banana", &none, ""));
        assert!(combobox_matches("Banana", &none, "   "));
        assert!(combobox_matches("Banana", &none, "ban"));
        assert!(combobox_matches("Banana", &none, "BAN"), "case-folded");
        assert!(
            combobox_matches("Banana", &none, "bnn"),
            "a subsequence, not a substring"
        );
        assert!(!combobox_matches("Banana", &none, "nab"), "order matters");
        assert!(!combobox_matches("Banana", &none, "bananas"));

        // Keywords widen the haystack, and the needle may span the join.
        let keywords = [String::from("berry"), String::from("blue")];
        assert!(combobox_matches("Blueberry", &keywords, "berry"));
        assert!(combobox_matches("Blueberry", &keywords, "blueblue"));
        assert!(!combobox_matches("Blueberry", &keywords, "zz"));
    }

    #[test]
    fn a_query_narrows_the_visible_set_and_clearing_it_restores_them() {
        let all = options();
        assert_eq!(visible_count(&all, ""), 3);
        assert_eq!(visible_count(&all, "b"), 2, "Banana and Blueberry");
        assert_eq!(visible_count(&all, "cher"), 1);
        assert_eq!(visible_count(&all, "zzz"), 0);
    }

    // ---- The panel --------------------------------------------------------

    #[test]
    fn the_panel_sizes_itself_to_the_rows_the_query_keeps() {
        let mut w = build::<App, _>(&panel_view("", None));
        let all = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert_eq!(all.height, LIST_PADDING * 2.0 + 3.0 * ITEM_HEIGHT);

        // Filtering to one row collapses the other two — once their lanes have
        // run, which the paint arm drives.
        requery(&mut w, "", "cher");
        paint_at(&mut w, all, 0.0);
        paint_at(&mut w, all, 5_000.0);
        let one = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert_eq!(one.height, LIST_PADDING * 2.0 + ITEM_HEIGHT);

        // Clearing it grows them back.
        requery(&mut w, "cher", "");
        paint_at(&mut w, one, 5_000.0);
        paint_at(&mut w, one, 10_000.0);
        let back = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert_eq!(back.height, all.height);
    }

    /// A filter change is animated, not cut: mid-collapse a leaving row still
    /// has height and a partial alpha, and the panel keeps asking for layout.
    #[test]
    fn a_leaving_row_collapses_rather_than_vanishing() {
        let mut w = build::<App, _>(&panel_view("", None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        requery(&mut w, "", "cher");

        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0));
        w.paint(&mut ctx, &mut rec);
        assert!(ctx.needs_layout(), "an animating list re-lays itself out");

        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(60.0));
        w.paint(&mut ctx, &mut rec);
        let leaving = w.rows[0].presence.value();
        assert!(
            leaving > 0.0 && leaving < 1.0,
            "row 0 is mid-collapse: {leaving}"
        );
        assert!(
            rec.layers.iter().any(|a| *a > 0.0 && *a < 1.0),
            "a collapsing row paints at a partial alpha: {:?}",
            rec.layers
        );

        // Long past the run it is gone and nothing more is owed.
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(5_000.0));
        w.paint(&mut ctx, &mut Recorder::default());
        assert_eq!(w.rows[0].presence.value(), 0.0);
        assert!(!ctx.needs_layout(), "a settled list owes no layout");
    }

    #[test]
    fn a_query_that_keeps_nothing_shows_the_empty_row_instead() {
        let mut w = build::<App, _>(&panel_view("zzz", None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert_eq!(size.height, LIST_PADDING * 2.0 + EMPTY_HEIGHT);
        assert!(w.row_rect(0).is_none(), "no row has any extent");
        assert!(w.empty_rect().is_some());
    }

    #[test]
    fn only_a_row_still_in_the_set_can_be_committed() {
        let mut w = build::<App, _>(&panel_view("", None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        let mut app = App::default();

        let row = w.row_rect(1).expect("row 1").center();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, row.x, row.y),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, row.x, row.y),
        );
        assert_eq!(
            app.picked,
            vec![1],
            "the index is into the whole option list"
        );

        // The disabled row is not a hit even at full presence.
        let row = w.row_rect(2).expect("row 2").center();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                size,
                &pointer(PointerPhase::Down, row.x, row.y)
            ),
            EventResult::Ignored
        );

        // Nor is a row on its way out of the set.
        requery(&mut w, "", "cher");
        paint_at(&mut w, size, 0.0);
        paint_at(&mut w, size, 30.0);
        let leaving = w.row_rect(0).expect("row 0 is still collapsing").center();
        assert_eq!(
            dispatch(
                &mut w,
                &mut app,
                size,
                &pointer(PointerPhase::Down, leaving.x, leaving.y)
            ),
            EventResult::Ignored
        );
        assert_eq!(app.picked, vec![1]);
    }

    #[test]
    fn the_panel_is_never_narrower_than_its_trigger() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::from_origin_size(Point::ZERO, Size::new(380.0, 44.0)));
        let view = ComboboxPanelView::<App> {
            options: options(),
            selected: None,
            query: String::new(),
            config: Rc::new(RefCell::new(PanelConfig {
                open: true,
                anchor: Some(anchor),
            })),
            on_select: Rc::new(|_: &mut App, _| {}),
        };
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(600.0, 600.0)));
        assert_eq!(size.width, 380.0);
    }

    /// A `Move` latches the row under the pointer; a move off every row, or a
    /// paint carrying no hover path, drops it again.
    #[test]
    fn a_move_latches_one_hovered_row_and_a_paint_without_hover_clears_it() {
        let mut w = build::<App, _>(&panel_view("", Some(0)));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        let mut app = App::default();
        let row = w.row_rect(1).expect("row 1").center();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Move, row.x, row.y),
        );
        assert_eq!(w.hovered, Some(1));

        // A move off every row drops the latch...
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Move, 1.0, 1.0),
        );
        assert_eq!(w.hovered, None);

        // ...and so does a paint with no hover path, which is the
        // self-correction every control in this catalog performs.
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Move, row.x, row.y),
        );
        assert_eq!(w.hovered, Some(1));
        let colors = panel_colors(None);
        let rec = paint_at(&mut w, size, 0.0);
        assert_eq!(w.hovered, None, "the paint pass corrected the latch");
        assert!(
            !rec.rrects.iter().any(
                |(_, _, radius, color)| *radius == select::ITEM_RADIUS && *color == colors.wash
            ),
            "an unhovered list washes nothing"
        );
    }

    /// The selection is an ink-and-check treatment, not a wash — upstream
    /// reserves the wash for the *active* row.
    #[test]
    fn the_selected_row_draws_its_check() {
        let mut w = build::<App, _>(&panel_view("", Some(0)));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        let mut checks = Vec::new();
        let mut rec = CheckRecorder {
            widths: &mut checks,
        };
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0));
        w.paint(&mut ctx, &mut rec);
        let expected = select::check_stroke_width(select::CHECK_SIZE);
        assert_eq!(
            checks
                .iter()
                .filter(|width| (**width - expected).abs() < 1e-9)
                .count(),
            1,
            "exactly the selected row drew a check"
        );
    }

    #[test]
    fn a_long_list_stops_at_the_height_cap() {
        let many: Vec<ComboboxOption> = (0..40)
            .map(|i| combobox_option(format!("row {i}")))
            .collect();
        let view = ComboboxPanelView::<App> {
            options: many,
            selected: None,
            query: String::new(),
            config: Rc::new(RefCell::new(PanelConfig {
                open: true,
                anchor: None,
            })),
            on_select: Rc::new(|_: &mut App, _| {}),
        };
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 900.0)));
        assert_eq!(size.height, COMBOBOX_MAX_HEIGHT);
    }

    #[test]
    fn reduce_motion_lands_every_row_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut w = build::<App, _>(&panel_view("", None));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        requery(&mut w, "", "cher");
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut Recorder::default());
        assert_eq!(w.rows[0].presence.value(), 0.0, "collapsed on the spot");
        assert!(!ctx.needs_layout());
    }

    // ---- The trigger ------------------------------------------------------

    #[test]
    fn the_trigger_shows_the_query_while_open_and_the_selection_while_closed() {
        let anchor = OverlayAnchor::new();
        let closed = combobox_trigger::<App, _>(&anchor, "ban", |_: &mut App, _| {})
            .selected_label(Some("Banana".into()));
        assert_eq!(closed.field_text(), "Banana");
        let open = combobox_trigger::<App, _>(&anchor, "ban", |_: &mut App, _| {})
            .selected_label(Some("Banana".into()))
            .open(true);
        assert_eq!(open.field_text(), "ban");
    }

    #[test]
    fn a_press_on_a_closed_trigger_asks_to_open_and_an_open_one_does_not() {
        let anchor = OverlayAnchor::new();
        let view = combobox_trigger::<App, _>(&anchor, "", |s: &mut App, t| s.query = t)
            .on_open_change(|s: &mut App, open| {
                s.open = open;
                s.opens.push(open);
            });
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        assert_eq!(size.height, TRIGGER_HEIGHT, "the catalog's field rung");

        let mut app = App::default();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 30.0, 20.0),
        );
        assert_eq!(app.opens, vec![true]);

        // Already open: the field keeps the press, and no second request goes out.
        w.open = true;
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 30.0, 20.0),
        );
        assert_eq!(app.opens, vec![true], "opening is reported once");
    }

    #[test]
    fn a_disabled_trigger_asks_for_nothing() {
        let anchor = OverlayAnchor::new();
        let view = combobox_trigger::<App, _>(&anchor, "", |_: &mut App, _| {})
            .disabled(true)
            .on_open_change(|s: &mut App, open| s.opens.push(open));
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 200.0)));
        let mut app = App::default();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 30.0, 20.0),
        );
        assert!(app.opens.is_empty());
    }

    #[test]
    fn the_trigger_captures_its_window_rect_into_the_anchor() {
        let anchor = OverlayAnchor::new();
        let view = combobox_trigger::<App, _>(&anchor, "", |_: &mut App, _| {});
        let mut w = build::<App, _>(&view);
        let size = Size::new(260.0, TRIGGER_HEIGHT);
        layout(&mut w, &BoxConstraints::tight(size));
        paint_at(&mut w, size, 0.0);
        assert_eq!(anchor.rect(), Rect::from_origin_size(Point::ZERO, size));
    }

    // ---- The mounted pair -------------------------------------------------

    #[test]
    fn the_public_builders_construct_and_dismiss_through_the_overlay_seam() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(20.0, 20.0, 240.0, 64.0));
        let view = combobox::<App, _>(options(), Some(0), "b", |s: &mut App, i| s.picked.push(i))
            .anchor(&anchor)
            .side(OverlaySide::Bottom)
            .align(OverlayAlign::Start)
            .open(true)
            .on_open_change(|s: &mut App, open| s.opens.push(open));
        assert_eq!(view.placement.offset, PANEL_OFFSET);

        let mut host = build::<App, _>(&view);
        let area = Size::new(500.0, 500.0);
        layout(&mut host, &BoxConstraints::tight(area));
        assert!(host.is_open());

        let mut app = App::default();
        dispatch(
            &mut host,
            &mut app,
            area,
            &pointer(PointerPhase::Down, 490.0, 490.0),
        );
        assert_eq!(app.opens, vec![false], "one dismissal, from the seam");
        let inside = host.content_rect().center();
        dispatch(
            &mut host,
            &mut app,
            area,
            &pointer(PointerPhase::Down, inside.x, inside.y),
        );
        assert_eq!(app.opens, vec![false]);
    }
}
