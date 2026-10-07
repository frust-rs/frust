//! Ports beUI's **Multi Select** — the token field whose chips pop in, wipe out
//! and wrap, over the same searchable panel the combobox opens.
//!
//! Source: `components/motion/multi-select/{index,context,trigger,content,list}.tsx`
//! (beUI v2, rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01), registry slug `multi-select`: *"Composable multi-select
//! primitives with searchable options, removable animated tokens, and a morphing
//! collision-aware panel."*
//!
//! | class / prop | here |
//! |---|---|
//! | trigger `min-h-11 min-w-52 rounded-xl border px-2.5 py-1.5` | [`TRIGGER_MIN_HEIGHT`], [`TRIGGER_MIN_WIDTH`], [`select::TRIGGER_RADIUS`], [`TRIGGER_PADDING_X`] / [`TRIGGER_PADDING_Y`] |
//! | chip row `flex-wrap gap-1.5` | the wrap packing, [`CHIP_GAP`] apart |
//! | chip `h-7 rounded-lg bg-muted px-2 text-xs font-medium` | [`CHIP_HEIGHT`], [`CHIP_RADIUS`], `surface_container_highest`, [`style::TEXT_XS`] |
//! | chip enter `y 6 → 0, scale 0.92 → 1, opacity 0 → 1` + `SPRING_SWAP` | [`CHIP_RISE`], [`CHIP_ENTER_SCALE`] |
//! | chip exit `clipPath inset(0 0 0 0% → 100%)`, `duration 0.16` | the leading-edge wipe, [`CHIP_EXIT_MS`] |
//! | remove `size-5 rounded-md`, `X size-3` | [`REMOVE_BOX`], [`REMOVE_RADIUS`], [`REMOVE_GLYPH`] |
//! | input `h-7 min-w-12 bg-transparent text-sm` | [`INPUT_HEIGHT`], [`INPUT_MIN_WIDTH`] |
//! | `ChevronsUpDown size-4 shrink-0` | [`select::CHEVRON_BOX`] |
//! | panel `rounded-xl border bg-background`, list `max-h-64 p-1.5` | [`select::PANEL_RADIUS`], [`combobox::COMBOBOX_MAX_HEIGHT`], [`combobox::LIST_PADDING`] |
//! | `defaultFilter` | [`combobox::combobox_matches`] — upstream's is the same function |
//!
//! # Shape of the port
//!
//! The two halves an app mounts, as everywhere else in this family:
//!
//! * [`multi_select_trigger`] — the token field: the chips for the selected
//!   options, their remove buttons, the search field, the chevrons, and the
//!   window-rect capture the panel is placed against.
//! * [`multi_select`] — the filtered panel, whose rows *toggle* rather than
//!   commit, so it stays open across a change.
//!
//! # Where the field chrome comes from
//!
//! Unlike [`crate::components::combobox`], whose trigger *is* a beUI field and
//! therefore wraps [`crate::components::input`] whole, this trigger has to hold
//! chips as well as an editable — so it takes the other route that component
//! documents: it paints the field's border and ring itself through `input`'s
//! shared `input`'s `FieldChrome` resolution, and hosts a bare
//! [`frust::text_input`] with that baseline's own chrome suppressed. Both routes
//! keep the editing the framework's; neither re-derives it.
//!
//! # Chips leave the flow the frame they are removed
//!
//! Upstream's chip list is an `AnimatePresence mode="popLayout"`: a removed chip
//! is taken out of layout immediately and wipes away where it stood while its
//! neighbours close the gap. The port does the same — [`layout`](MultiSelectTriggerWidget)
//! packs only the live chips, and a leaving one keeps painting at the rect its
//! last layout gave it until its wipe finishes.
//!
//! # v1 boundaries, adopted from the sibling catalog
//!
//! * **No type-ahead, and no keyboard list navigation.** Same reason as
//!   [`crate::components::combobox`]: the wrapped editable owns the focus path,
//!   and there is no seam to forward its declined keys to a sibling overlay.
//!   That also drops upstream's Backspace-removes-the-last-chip shortcut, which
//!   is a key the `<input>` sees.
//! * **The panel's height cap is a constant** ([`combobox::COMBOBOX_MAX_HEIGHT`]),
//!   and a longer list clips rather than scrolls.
//!
//! # Degradations
//!
//! - **Neighbouring chips close the gap immediately.** Upstream springs each
//!   surviving chip to its new position (`layout="position"` on
//!   [`SPRING_LAYOUT`]); here the wrap
//!   packing re-runs and the remaining chips cut to their new slots. The
//!   leaving chip's own wipe is ported.
//! - **The wipe is a rectangular clip, not a rounded one.** `clipPath: inset(…
//!   round 0.5rem)` carries the chip's own radius; the scene's clip stack takes
//!   a rounded push, but the *animated* edge here is a straight one, so the
//!   wipe is pushed as a plain rect and the chip's corners are the fill's.
//! - **The hosted editable paints its own opaque background.** The baseline
//!   field offers no seam to suppress its fill, so the search slot inside the
//!   token field is a `surface`-filled rounded rect rather than the transparent
//!   one upstream draws. beUI folds `surface` onto `--background`, so the two
//!   agree wherever the control sits on the page background — `input`'s own
//!   documented degradation, inherited.
//! - **No group headings, labels or separators** in the panel, and **no active-row
//!   highlight travel** — both as [`crate::components::combobox`] records.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{Theme, text_input};

use super::combobox::{self, combobox_matches};
use super::input::{FieldChrome, paint_field_frame, resolve_field_paint};
use super::select::{self, PANEL_RADIUS, draw_check, draw_chevron, panel_colors, stroke_frame};
use crate::motion::Ramp;
use crate::overlay::anchored::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::press::{Lane, inside_inclusive, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, label_style};
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT, SPRING_SWAP};

// ---- Metrics ---------------------------------------------------------------

/// The token field's minimum height, in logical px (`min-h-11`).
pub const TRIGGER_MIN_HEIGHT: f64 = style::HEIGHT_INPUT;

/// The token field's minimum width, in logical px (`min-w-52`).
pub const TRIGGER_MIN_WIDTH: f64 = 208.0;

/// Horizontal padding inside the token field, in logical px (`px-2.5`).
pub const TRIGGER_PADDING_X: f64 = 10.0;

/// Vertical padding inside the token field, in logical px (`py-1.5`).
pub const TRIGGER_PADDING_Y: f64 = 6.0;

/// Gap between the chip area and the chevrons, in logical px (`gap-2`).
pub const TRIGGER_GAP: f64 = style::GAP_MD;

/// Gap between two chips, in logical px (`gap-1.5`).
pub const CHIP_GAP: f64 = style::GAP_SM;

/// A chip's height, in logical px (`h-7`).
pub const CHIP_HEIGHT: f64 = 28.0;

/// A chip's corner radius, in logical px (`rounded-lg`).
pub const CHIP_RADIUS: f64 = style::RADIUS_LG;

/// Horizontal padding inside a chip, in logical px (`px-2`).
pub const CHIP_PADDING_X: f64 = 8.0;

/// Gap between a chip's label and its remove button, in logical px (`gap-1`).
pub const CHIP_INNER_GAP: f64 = 4.0;

/// The remove button's box, in logical px (`size-5`).
pub const REMOVE_BOX: f64 = 20.0;

/// The remove button's corner radius, in logical px (`rounded-md`).
pub const REMOVE_RADIUS: f64 = style::RADIUS_MD;

/// The remove glyph's box, in logical px (`X size-3`).
pub const REMOVE_GLYPH: f64 = 12.0;

/// How far the remove button hangs into the chip's trailing padding, in logical
/// px (`-mr-1`).
pub const REMOVE_PULL: f64 = 4.0;

/// The search slot's height, in logical px (`h-7`).
pub const INPUT_HEIGHT: f64 = 28.0;

/// The search slot's minimum width before it wraps to its own row, in logical px
/// (`min-w-12`).
pub const INPUT_MIN_WIDTH: f64 = 48.0;

/// How far an entering chip rises into place, in logical px
/// (`translateY(6px) → 0`).
pub const CHIP_RISE: f64 = 6.0;

/// The scale an entering chip grows from (`scale(0.92) → 1`).
pub const CHIP_ENTER_SCALE: f64 = 0.92;

/// How long a leaving chip's wipe takes, in milliseconds (`duration: 0.16`).
pub const CHIP_EXIT_MS: u64 = 160;

/// Opacity of a disabled control: `disabled:opacity-50`.
const DISABLED_OPACITY: f32 = style::DISABLED_OPACITY_INPUT;

/// Opacity of a disabled row in the panel: `disabled:opacity-45`.
const ROW_DISABLED_OPACITY: f32 = 0.45;

/// The type-scale role a chip's label takes its family from at layout — a
/// small token label. The panel's rows are [`select::LABEL_ROLE`] and its
/// empty state [`combobox::EMPTY_ROLE`], as on the combobox.
const CHIP_ROLE: ThemeTextType = ThemeTextType::LabelSmall;

// ---- Options ---------------------------------------------------------------

/// One option in a [`multi_select`] list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultiSelectOption {
    label: String,
    keywords: Vec<String>,
    disabled: bool,
}

/// Create an option labelled `label`.
pub fn multi_select_option(label: impl Into<String>) -> MultiSelectOption {
    MultiSelectOption {
        label: label.into(),
        keywords: Vec::new(),
        disabled: false,
    }
}

impl MultiSelectOption {
    /// Add search terms the option also matches on, beyond its own label.
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

    /// Whether `query` keeps this option — [`combobox::combobox_matches`],
    /// which is the same `defaultFilter` upstream shares between the two
    /// components.
    pub fn matches(&self, query: &str) -> bool {
        combobox_matches(&self.label, &self.keywords, query)
    }
}

// ---- The trigger -----------------------------------------------------------

/// A view-held, typed text callback (erased by the wrapped editable).
type OnText<State> = Rc<dyn Fn(&mut State, String)>;
/// A view-held, typed index callback (erased on build).
type OnIndex<State> = Rc<dyn Fn(&mut State, usize)>;
/// A view-held, typed open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative beUI multi-select trigger. See [`multi_select_trigger`].
pub struct MultiSelectTriggerView<State: 'static> {
    anchor: OverlayAnchor,
    options: Vec<MultiSelectOption>,
    selected: Vec<usize>,
    query: String,
    placeholder: String,
    open: bool,
    disabled: bool,
    on_query_change: OnText<State>,
    on_remove: Option<OnIndex<State>>,
    on_open_change: OnOpenChange<State>,
}

/// Create the multi-select's trigger: the token field showing a chip per entry
/// of `selected`, the search field holding `query`, and the window-rect capture
/// the panel is anchored to.
///
/// `selected` holds indices into `options`; every callback reports one of those
/// same indices, so an app never maps a chip back itself.
pub fn multi_select_trigger<State: 'static, F: Fn(&mut State, String) + 'static>(
    anchor: &OverlayAnchor,
    options: Vec<MultiSelectOption>,
    selected: Vec<usize>,
    query: impl Into<String>,
    on_query_change: F,
) -> MultiSelectTriggerView<State> {
    MultiSelectTriggerView {
        anchor: anchor.clone(),
        options,
        selected,
        query: query.into(),
        placeholder: String::from("Search…"),
        open: false,
        disabled: false,
        on_query_change: Rc::new(on_query_change),
        on_remove: None,
        on_open_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> MultiSelectTriggerView<State> {
    /// Set the search field's placeholder. Upstream hides it once anything is
    /// selected (`placeholder={values.length ? "" : placeholder}`), and so does
    /// this port.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Tell the trigger whether the panel is open — whether a press asks to open.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Disable the control: dimmed chrome, inert chips, no open requests.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Set the chip-removal callback: a release on a chip's × reports that
    /// chip's option index.
    pub fn on_remove<F: Fn(&mut State, usize) + 'static>(mut self, on_remove: F) -> Self {
        self.on_remove = Some(Rc::new(on_remove));
        self
    }

    /// Set the open-request callback: a press inside the field reports `true`.
    ///
    /// Only ever `true` — a *close* comes from the overlay seam's dismissal.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }

    /// The chips this trigger shows: `(option index, label)`, in selection order,
    /// skipping any index the option list does not have.
    fn chips(&self) -> Vec<(usize, String)> {
        self.selected
            .iter()
            .filter_map(|index| {
                self.options
                    .get(*index)
                    .map(|option| (*index, option.label.clone()))
            })
            .collect()
    }

    /// The hosted editable, with the baseline's own chrome suppressed.
    // erasure: keep feeds build_child/rebuild_child/teardown_child, which take &AnyView
    fn control(&self) -> AnyView<State> {
        let on_change = self.on_query_change.clone();
        let placeholder = if self.selected.is_empty() {
            self.placeholder.clone()
        } else {
            String::new()
        };
        // The query and placeholder keep the baseline `text_input`'s own
        // family: that field has no themed-family seam.
        any(
            text_input(self.query.clone(), move |state: &mut State, text| {
                on_change(state, text);
            })
            .placeholder(placeholder)
            .enabled(!self.disabled)
            .padding(0.0, 0.0)
            .border_width(0.0)
            .corner_radius(CHIP_RADIUS),
        )
    }
}

/// One chip's retained state.
struct ChipCell {
    /// The option index this chip stands for — its identity across rebuilds.
    index: usize,
    text: LabelRun,
    /// `0.0` gone .. `1.0` fully present. Entering runs on [`SPRING_SWAP`],
    /// leaving on the short wipe ramp (see [`ChipCell::leave`]).
    presence: Lane,
    /// Whether this chip is on its way out and no longer takes layout space.
    leaving: bool,
    /// The box its last layout gave it — what a leaving chip keeps wiping in.
    rect: Rect,
}

impl ChipCell {
    /// A fresh chip, entering from nothing.
    fn entering(index: usize, label: &str) -> Self {
        let mut cell = ChipCell {
            index,
            text: LabelRun::new(label),
            presence: Lane::at_rest(Ramp::spring(SPRING_SWAP), 0.0),
            leaving: false,
            rect: Rect::ZERO,
        };
        cell.presence.retarget(1.0);
        cell
    }

    /// A chip already at rest — how the first build seeds an app's initial
    /// selection, which must not play an entrance nobody asked for.
    fn at_rest(index: usize, label: &str) -> Self {
        ChipCell {
            index,
            text: LabelRun::new(label),
            presence: Lane::at_rest(Ramp::spring(SPRING_SWAP), 1.0),
            leaving: false,
            rect: Rect::ZERO,
        }
    }

    /// Take the chip out of the flow and start its wipe.
    fn leave(&mut self) {
        self.leaving = true;
        self.presence.retarget_with(
            Ramp::eased(Duration::from_millis(CHIP_EXIT_MS), EASE_OUT),
            0.0,
        );
    }

    /// The remove button's box inside a chip at `rect`.
    fn remove_rect(rect: Rect) -> Rect {
        Rect::from_origin_size(
            Point::new(
                rect.x1 - CHIP_PADDING_X + REMOVE_PULL - REMOVE_BOX,
                rect.y0 + (rect.height() - REMOVE_BOX) / 2.0,
            ),
            Size::new(REMOVE_BOX, REMOVE_BOX),
        )
    }
}

/// The retained widget for a [`MultiSelectTriggerView`].
pub struct MultiSelectTriggerWidget {
    /// The hosted editable — the whole of the editing surface.
    field: ChildPod,
    anchor: OverlayAnchor,
    chips: Vec<ChipCell>,
    open: bool,
    disabled: bool,
    /// The field's focus crossfade, driving the border and ring exactly as
    /// `input`'s does.
    focus: Lane,
    was_focused: bool,
    /// The chip whose × a `Down` armed.
    captured_remove: Option<usize>,
    /// The chip whose × the pointer is over.
    hovered_remove: Option<usize>,
    /// This widget's own measured size, for the wrap packing.
    size: Size,
    on_remove: Option<ErasedArgCallback<usize>>,
    on_open_change: ErasedArgCallback<bool>,
}

impl MultiSelectTriggerWidget {
    /// The chips that still take layout space.
    fn live(&self) -> impl Iterator<Item = &ChipCell> {
        self.chips.iter().filter(|chip| !chip.leaving)
    }

    /// The chip whose × contains panel-local `pos`, if any.
    fn hit_remove(&self, pos: Point) -> Option<usize> {
        self.chips
            .iter()
            .position(|chip| !chip.leaving && ChipCell::remove_rect(chip.rect).contains(pos))
    }

    /// Reconcile the retained chips against `chips` (option index + label),
    /// reporting whether anything moved.
    ///
    /// Order is preserved rather than rebuilt: a leaving chip keeps its slot in
    /// the vector so it goes on painting where it stood, and a new one is
    /// appended — which is where a fresh selection lands upstream too.
    fn reconcile(&mut self, chips: &[(usize, String)], first_build: bool) -> bool {
        // A settled exit is what actually drops a cell; doing it here rather
        // than in paint keeps the vector stable for the whole of a frame.
        let before = self.chips.len();
        self.chips
            .retain(|chip| !(chip.leaving && chip.presence.value() <= 0.0));
        let mut changed = self.chips.len() != before;

        for chip in &mut self.chips {
            let wanted = chips.iter().find(|(index, _)| *index == chip.index);
            match wanted {
                Some((_, label)) => {
                    if chip.leaving {
                        // Re-selected mid-exit: put it back in the flow rather
                        // than leaving a ghost beside its own fresh copy.
                        chip.leaving = false;
                        chip.presence.retarget_with(Ramp::spring(SPRING_SWAP), 1.0);
                        changed = true;
                    }
                    if chip.text.set_content(label.clone()) {
                        changed = true;
                    }
                }
                None => {
                    if !chip.leaving {
                        chip.leave();
                        changed = true;
                    }
                }
            }
        }
        for (index, label) in chips {
            if self.chips.iter().any(|chip| chip.index == *index) {
                continue;
            }
            self.chips.push(if first_build {
                ChipCell::at_rest(*index, label)
            } else {
                ChipCell::entering(*index, label)
            });
            changed = true;
        }
        changed
    }
}

impl<State: 'static> View<State> for MultiSelectTriggerView<State> {
    type Element = MultiSelectTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MultiSelectTriggerWidget {
        let mut element = MultiSelectTriggerWidget {
            field: build_child(&self.control(), ctx),
            anchor: self.anchor.clone(),
            chips: Vec::new(),
            open: self.open,
            disabled: self.disabled,
            focus: Lane::at_rest(super::switch::TRACK_COLOR_RAMP, 0.0),
            was_focused: false,
            captured_remove: None,
            hovered_remove: None,
            size: Size::ZERO,
            on_remove: self.on_remove.as_ref().map(erase_callback_arg),
            on_open_change: erase_callback_arg(&self.on_open_change),
        };
        element.reconcile(&self.chips(), true);
        element
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MultiSelectTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_remove = self.on_remove.as_ref().map(erase_callback_arg);
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        element.anchor = self.anchor.clone();
        element.open = self.open;
        let mut flags = rebuild_child(&prev.control(), &self.control(), &mut element.field, ctx);
        if element.disabled != self.disabled {
            element.disabled = self.disabled;
            if self.disabled {
                element.captured_remove = None;
                element.hovered_remove = None;
            }
            flags |= ChangeFlags::PAINT;
        }
        if element.reconcile(&self.chips(), false) {
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut MultiSelectTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.control(), &mut element.field, ctx);
    }
}

impl Widget for MultiSelectTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(style::TEXT_XS);
        for chip in &mut self.chips {
            chip.text.layout_themed(ctx, &style, CHIP_ROLE);
        }

        let width = if bc.max().width.is_finite() {
            bc.max().width.max(TRIGGER_MIN_WIDTH)
        } else {
            TRIGGER_MIN_WIDTH
        };
        // The chevrons are `shrink-0` at the trailing end, outside the wrapping
        // token area.
        let field_left = TRIGGER_PADDING_X;
        let field_right = width - TRIGGER_PADDING_X - select::CHEVRON_BOX - TRIGGER_GAP;
        let field_width = (field_right - field_left).max(0.0);

        // `flex-wrap`: fill a row, then drop to the next.
        let mut x = 0.0;
        let mut y = 0.0;
        let mut rows = 1.0;
        let indices: Vec<usize> = (0..self.chips.len())
            .filter(|i| !self.chips[*i].leaving)
            .collect();
        for index in indices {
            let text = self.chips[index].text.size().width;
            let chip_width =
                CHIP_PADDING_X * 2.0 + text + CHIP_INNER_GAP + REMOVE_BOX - REMOVE_PULL;
            if x > 0.0 && x + chip_width > field_width {
                x = 0.0;
                y += CHIP_HEIGHT + CHIP_GAP;
                rows += 1.0;
            }
            self.chips[index].rect = Rect::from_origin_size(
                Point::new(field_left + x, TRIGGER_PADDING_Y + y),
                Size::new(chip_width, CHIP_HEIGHT),
            );
            x += chip_width + CHIP_GAP;
        }

        // The search slot takes what is left of the last row, or a row of its own.
        let remaining = field_width - x;
        if x > 0.0 && remaining < INPUT_MIN_WIDTH {
            x = 0.0;
            y += CHIP_HEIGHT + CHIP_GAP;
            rows += 1.0;
        }
        let slot = Rect::from_origin_size(
            Point::new(
                field_left + x,
                TRIGGER_PADDING_Y + y + (CHIP_HEIGHT - INPUT_HEIGHT) / 2.0,
            ),
            Size::new((field_width - x).max(INPUT_MIN_WIDTH), INPUT_HEIGHT),
        );
        self.field
            .layout_child(ctx, &BoxConstraints::tight(slot.size()));
        self.field.set_origin(slot.origin());

        let content = TRIGGER_PADDING_Y * 2.0 + rows * CHIP_HEIGHT + (rows - 1.0) * CHIP_GAP;
        self.size = Size::new(width, content.max(TRIGGER_MIN_HEIGHT));
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The pod's focus path is authoritative, exactly as `input` reads it.
        let focused = ctx.has_focus() && !self.disabled;
        if focused != self.was_focused {
            self.was_focused = focused;
            self.focus.retarget(if focused { 1.0 } else { 0.0 });
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = panel_colors(theme);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        self.anchor.set(Rect::from_origin_size(origin, size));

        let mut owes_frame = false;
        let mut settled_exit = false;
        if reduce {
            self.focus.snap();
            for chip in &mut self.chips {
                chip.presence.snap();
            }
        } else {
            owes_frame |= self.focus.advance(now);
            for chip in &mut self.chips {
                owes_frame |= chip.presence.advance(now);
            }
        }
        // A wipe that has just finished has to reach layout, which is what drops
        // the cell (see `reconcile`).
        for chip in &self.chips {
            settled_exit |= chip.leaving && chip.presence.value() <= 0.0;
        }

        // The field's own chrome, resolved through `input`'s shared ladder.
        let paint = resolve_field_paint(
            theme,
            FieldChrome {
                focused,
                error: false,
                disabled: self.disabled,
            },
            self.focus.value().clamp(0.0, 1.0),
            0.0,
        );

        self.field.paint_child(ctx, scene);

        let tint = |color: Color| style::disabled_tint(color, self.disabled, DISABLED_OPACITY);
        for chip in &self.chips {
            let presence = chip.presence.value();
            if presence <= 0.0 {
                continue;
            }
            let rect = chip.rect + origin.to_vec2();
            if chip.leaving {
                // `clipPath: inset(0 0 0 100%)` — the chip is wiped away from its
                // leading edge, keeping full opacity as it goes.
                let cut = rect.width() * (1.0 - presence.clamp(0.0, 1.0));
                scene.push_clip(
                    Point::new(rect.x0 + cut, rect.y0),
                    Size::new((rect.width() - cut).max(0.0), rect.height()),
                );
            } else if presence < 1.0 {
                // The entrance is a rise, a shrink and a fade, about the chip's
                // own centre.
                let alpha = presence.clamp(0.0, 1.0);
                let scale = CHIP_ENTER_SCALE + (1.0 - CHIP_ENTER_SCALE) * alpha;
                let centre = rect.center();
                scene.push_layer(rect.origin(), rect.size(), alpha as f32);
                scene.push_transform(
                    frust::authoring::Affine::translate(centre.to_vec2())
                        * frust::authoring::Affine::scale(scale)
                        * frust::authoring::Affine::translate(-centre.to_vec2())
                        * frust::authoring::Affine::translate((0.0, CHIP_RISE * (1.0 - alpha))),
                );
            }

            scene.fill_rounded_rect(rect.origin(), rect.size(), CHIP_RADIUS, tint(colors.wash));
            let text = chip.text.size();
            chip.text.paint(
                Point::new(
                    rect.x0 + CHIP_PADDING_X,
                    rect.y0 + (rect.height() - text.height) / 2.0,
                ),
                tint(colors.ink),
                scene,
            );

            let remove = ChipCell::remove_rect(rect);
            let armed = self
                .hovered_remove
                .is_some_and(|index| self.chips.get(index).is_some_and(|c| c.index == chip.index));
            if armed && !self.disabled {
                scene.fill_rounded_rect(
                    remove.origin(),
                    remove.size(),
                    REMOVE_RADIUS,
                    style::with_alpha(colors.ink, style::HOVER_WASH_ALPHA * 2.0),
                );
            }
            draw_cross(
                scene,
                remove.center(),
                REMOVE_GLYPH,
                tint(if armed { colors.ink } else { colors.muted }),
            );

            if chip.leaving {
                scene.pop_clip();
            } else if presence < 1.0 {
                scene.pop_transform();
                scene.pop_layer();
            }
        }

        // `ChevronsUpDown`, `shrink-0` at the trailing edge.
        let centre = Point::new(
            origin.x + size.width - TRIGGER_PADDING_X - select::CHEVRON_BOX / 2.0,
            origin.y + size.height / 2.0,
        );
        let half = select::CHEVRON_BOX / 4.0;
        draw_chevron(
            scene,
            Point::new(centre.x, centre.y - half),
            select::CHEVRON_SPIN,
            tint(colors.muted),
            select::CHEVRON_BOX,
        );
        draw_chevron(
            scene,
            Point::new(centre.x, centre.y + half),
            0.0,
            tint(colors.muted),
            select::CHEVRON_BOX,
        );

        // Last, over the opaque child fill — the ordering `input` documents.
        paint_field_frame(scene, origin, size, paint);

        if settled_exit {
            // Dropping the cell shortens the wrap, which is a layout change.
            ctx.request_layout();
        } else if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The editable first — it owns the caret and the focus session.
        let routed = route_event_single(&mut self.field, ctx, event);
        if routed == EventResult::Handled {
            return routed;
        }
        let InputEvent::Pointer(p) = event else {
            return routed;
        };
        if self.disabled {
            return routed;
        }
        match p.phase {
            PointerPhase::Down => {
                if !presses(p) {
                    return routed;
                }
                // A press on a chip's × is the chip's, never an open request —
                // upstream stops that press from reaching the trigger wrapper.
                if let Some(index) = self.hit_remove(p.position) {
                    self.captured_remove = Some(index);
                    ctx.capture_pointer();
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if !self.open && inside_inclusive(p.position, ctx.size()) {
                    (self.on_open_change)(ctx, true);
                    return EventResult::Handled;
                }
                routed
            }
            PointerPhase::Move => {
                let hovered = self.hit_remove(p.position);
                if hovered.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered_remove != hovered {
                    self.hovered_remove = hovered;
                    ctx.request_redraw();
                }
                if self.captured_remove.is_some() {
                    return EventResult::Handled;
                }
                routed
            }
            PointerPhase::Up => {
                let Some(index) = self.captured_remove.take() else {
                    return routed;
                };
                if self.hit_remove(p.position) == Some(index)
                    && let Some(on_remove) = self.on_remove.as_mut()
                    && let Some(chip) = self.chips.get(index)
                {
                    let option = chip.index;
                    on_remove(ctx, option);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.captured_remove.take().is_none() {
                    return routed;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::ComboBox,
            |node| {
                node.set_expanded(self.open);
                if self.disabled {
                    node.set_disabled();
                }
            },
            |ctx| {
                self.field.semantics_child(ctx);
                for chip in self.live() {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(format!("Remove {}", chip.text.content()));
                        if self.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }

    visit_children!(field);
}

/// Paint a lucide `X` centred on `centre`, `size` on a side.
fn draw_cross(scene: &mut dyn PaintScene, centre: Point, size: f64, color: Color) {
    let arm = size / 2.0;
    let mut path = BezPath::new();
    path.move_to(Point::new(centre.x - arm, centre.y - arm));
    path.line_to(Point::new(centre.x + arm, centre.y + arm));
    path.move_to(Point::new(centre.x + arm, centre.y - arm));
    path.line_to(Point::new(centre.x - arm, centre.y + arm));
    scene.stroke_path(
        Point::ZERO,
        &path,
        style::BORDER_WIDTH * 1.5,
        &Brush::Solid(color),
    );
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

/// A declarative beUI multi-select panel. See [`multi_select`].
pub struct MultiSelectView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    config: PanelHandle,
    placement: OverlayPlacement,
}

/// Build a multi-select panel over `options`, filtered live by `query`, checking
/// every index in `selected`, and reporting a row press through
/// `on_toggle(state, option_index)`.
///
/// A press **toggles** rather than commits: the panel stays open, which is what
/// makes a multi-select usable in one gesture chain.
pub fn multi_select<State: 'static, F: Fn(&mut State, usize) + 'static>(
    options: Vec<MultiSelectOption>,
    selected: Vec<usize>,
    query: impl Into<String>,
    on_toggle: F,
) -> MultiSelectView<State> {
    let config: PanelHandle = Rc::new(RefCell::new(PanelConfig {
        open: true,
        ..PanelConfig::default()
    }));
    let content = MultiSelectPanelView {
        options,
        selected,
        query: query.into(),
        config: config.clone(),
        on_toggle: Rc::new(on_toggle),
    };
    let placement = OverlayPlacement::default()
        .align(OverlayAlign::Start)
        .offset(combobox::PANEL_OFFSET);
    MultiSelectView {
        inner: anchored(content).placement(placement),
        config,
        placement,
    }
}

impl<State: 'static> MultiSelectView<State> {
    /// Anchor the panel to the token field's captured rect — which is also where
    /// its minimum width comes from.
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

    /// Hand a **kept-mounted** panel the app's open flag. The default is `true`.
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

impl<State: 'static> View<State> for MultiSelectView<State> {
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

/// The panel's content view. Never mounted on its own; [`multi_select`] wraps it.
struct MultiSelectPanelView<State: 'static> {
    options: Vec<MultiSelectOption>,
    selected: Vec<usize>,
    query: String,
    config: PanelHandle,
    on_toggle: OnIndex<State>,
}

/// One filtered row's retained state.
struct FilterRow {
    text: LabelRun,
    disabled: bool,
    /// How present the row is under the current query — height and alpha both.
    presence: Lane,
}

/// The retained widget for the multi-select panel.
pub struct MultiSelectPanelWidget {
    rows: Vec<FilterRow>,
    selected: Vec<usize>,
    config: PanelHandle,
    empty: LabelRun,
    empty_presence: Lane,
    content: Size,
    hovered: Option<usize>,
    captured: Option<usize>,
    on_toggle: ErasedArgCallback<usize>,
}

/// The retained rows for `options`, each resting at its visibility under `query`.
fn rows_for(options: &[MultiSelectOption], query: &str) -> Vec<FilterRow> {
    options
        .iter()
        .map(|option| FilterRow {
            text: LabelRun::new(option.label.clone()),
            disabled: option.disabled,
            presence: Lane::at_rest(
                Ramp::spring(SPRING_LAYOUT),
                f64::from(u8::from(option.matches(query))),
            ),
        })
        .collect()
}

/// How many of `options` `query` keeps.
fn visible_count(options: &[MultiSelectOption], query: &str) -> usize {
    options.iter().filter(|o| o.matches(query)).count()
}

impl MultiSelectPanelWidget {
    /// Row `index`'s current height, in logical px.
    fn row_height(&self, index: usize) -> f64 {
        self.rows.get(index).map_or(0.0, |row| {
            combobox::ITEM_HEIGHT * row.presence.value().clamp(0.0, 1.0)
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
            Point::new(combobox::LIST_PADDING, combobox::LIST_PADDING + top),
            Size::new(
                (self.content.width - combobox::LIST_PADDING * 2.0).max(0.0),
                height,
            ),
        ))
    }

    /// The row under a panel-local `pos` — only one staying in the filtered set.
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
        rows + combobox::EMPTY_HEIGHT * self.empty_presence.value().clamp(0.0, 1.0)
    }

    /// Whether option `index` is one of the checked ones.
    fn is_checked(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }
}

impl<State: 'static> View<State> for MultiSelectPanelView<State> {
    type Element = MultiSelectPanelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MultiSelectPanelWidget {
        let empty = visible_count(&self.options, &self.query) == 0;
        MultiSelectPanelWidget {
            rows: rows_for(&self.options, &self.query),
            selected: self.selected.clone(),
            config: self.config.clone(),
            empty: LabelRun::new(combobox::EMPTY_LABEL),
            empty_presence: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), f64::from(u8::from(empty))),
            content: Size::ZERO,
            hovered: None,
            captured: None,
            on_toggle: erase_callback_arg(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MultiSelectPanelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = erase_callback_arg(&self.on_toggle);
        element.config = self.config.clone();
        let mut flags = ChangeFlags::NONE;
        if prev.options != self.options {
            element.rows = rows_for(&self.options, &self.query);
            element.hovered = None;
            element.captured = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else if prev.query != self.query {
            let mut moved = false;
            for (row, option) in element.rows.iter_mut().zip(&self.options) {
                let target = f64::from(u8::from(option.matches(&self.query)));
                if row.presence.target() != target {
                    row.presence.retarget(target);
                    moved = true;
                }
            }
            if moved {
                element.hovered = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        let empty = f64::from(u8::from(visible_count(&self.options, &self.query) == 0));
        if element.empty_presence.target() != empty {
            element.empty_presence.retarget(empty);
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.selected != self.selected {
            element.selected = self.selected.clone();
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for MultiSelectPanelWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(style::TEXT_SM);
        let mut widest: f64 = 0.0;
        for row in &mut self.rows {
            widest = widest.max(
                row.text
                    .layout_themed(ctx, &style, select::LABEL_ROLE)
                    .width,
            );
        }
        widest = widest.max(
            self.empty
                .layout_themed(ctx, &style, combobox::EMPTY_ROLE)
                .width,
        );

        let anchor_width = self
            .config
            .borrow()
            .anchor
            .as_ref()
            .map_or(0.0, |anchor| anchor.rect().width());
        let natural = widest
            + (combobox::ITEM_PADDING_X + combobox::LIST_PADDING) * 2.0
            + select::TRIGGER_GAP
            + select::CHECK_SIZE
            + style::BORDER_WIDTH * 2.0;
        let width = natural.max(anchor_width).min(bc.max().width);
        let height = (combobox::LIST_PADDING * 2.0 + self.list_height())
            .min(combobox::COMBOBOX_MAX_HEIGHT)
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

            let checked = self.is_checked(index);
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
                if hovered || checked {
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
                    rect.x0 + combobox::ITEM_PADDING_X,
                    rect.y0 + (rect.height() - text.height) / 2.0,
                ),
                ink,
                scene,
            );
            if checked {
                draw_check(
                    scene,
                    Point::new(
                        rect.x1 - combobox::ITEM_PADDING_X - select::CHECK_SIZE,
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

        if self.empty_presence.value() > 0.0 {
            let top: f64 = (0..self.rows.len()).map(|i| self.row_height(i)).sum();
            let presence = self.empty_presence.value().clamp(0.0, 1.0);
            let rect = Rect::from_origin_size(
                Point::new(
                    origin.x + combobox::LIST_PADDING,
                    origin.y + combobox::LIST_PADDING + top,
                ),
                Size::new(
                    (size.width - combobox::LIST_PADDING * 2.0).max(0.0),
                    combobox::EMPTY_HEIGHT * presence,
                ),
            );
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

        // The row heights *are* the panel's height, so a filtering list needs a
        // relayout rather than a bare frame.
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
                    (self.on_toggle)(ctx, index);
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
        ctx.push_container(
            Role::ListBox,
            |_| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    if row.presence.target() < 1.0 {
                        continue;
                    }
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(row.text.content());
                        node.set_selected(self.is_checked(index));
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
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
    }

    #[derive(Clone, Default)]
    struct App {
        query: String,
        selected: Vec<usize>,
        removed: Vec<usize>,
        toggled: Vec<usize>,
        opens: Vec<bool>,
    }

    fn ft_ms(millis: f64) -> FrameTime {
        FrameTime::from_nanos((millis * 1_000_000.0) as u64)
    }

    fn options() -> Vec<MultiSelectOption> {
        vec![
            multi_select_option("Rust"),
            multi_select_option("Ruby").keywords(["gem"]),
            multi_select_option("Zig").disabled(true),
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

    fn trigger_view(selected: Vec<usize>) -> MultiSelectTriggerView<App> {
        let anchor = OverlayAnchor::new();
        multi_select_trigger::<App, _>(&anchor, options(), selected, "", |s: &mut App, t| {
            s.query = t
        })
        .on_remove(|s: &mut App, index| s.removed.push(index))
        .on_open_change(|s: &mut App, open| s.opens.push(open))
    }

    /// Rebuild a trigger with a different selection, the way a rebuild would.
    fn reselect(w: &mut MultiSelectTriggerWidget, from: Vec<usize>, to: Vec<usize>) {
        let prev = trigger_view(from);
        let next = trigger_view(to);
        let mut counter = 0u64;
        View::<App>::rebuild(&next, &prev, w, &mut BuildCtx::new(&mut counter));
    }

    fn panel_view(selected: Vec<usize>, query: &str) -> MultiSelectPanelView<App> {
        MultiSelectPanelView {
            options: options(),
            selected,
            query: query.to_string(),
            config: Rc::new(RefCell::new(PanelConfig {
                open: true,
                anchor: None,
            })),
            on_toggle: Rc::new(|s: &mut App, i| s.toggled.push(i)),
        }
    }

    // ---- Metrics ----------------------------------------------------------

    #[test]
    fn the_authored_metrics_are_the_upstream_ones() {
        assert_eq!(TRIGGER_MIN_HEIGHT, 44.0, "min-h-11");
        assert_eq!(TRIGGER_MIN_WIDTH, 208.0, "min-w-52");
        assert_eq!(TRIGGER_PADDING_X, 10.0, "px-2.5");
        assert_eq!(TRIGGER_PADDING_Y, 6.0, "py-1.5");
        assert_eq!(CHIP_HEIGHT, 28.0, "h-7");
        assert_eq!(CHIP_GAP, 6.0, "gap-1.5");
        assert_eq!(CHIP_RADIUS, 8.0, "rounded-lg");
        assert_eq!(REMOVE_BOX, 20.0, "size-5");
        assert_eq!(INPUT_MIN_WIDTH, 48.0, "min-w-12");
        assert_eq!(CHIP_ENTER_SCALE, 0.92, "scale(0.92)");
        assert_eq!(CHIP_EXIT_MS, 160, "duration: 0.16");
    }

    /// The filter is shared with the combobox, not re-derived.
    #[test]
    fn an_option_matches_through_the_shared_filter() {
        let ruby = &options()[1];
        assert!(ruby.matches(""));
        assert!(ruby.matches("rb"), "a subsequence of the label");
        assert!(ruby.matches("gem"), "a keyword");
        assert!(!ruby.matches("zz"));
    }

    // ---- The token field --------------------------------------------------

    #[test]
    fn an_empty_field_is_one_row_tall_and_at_the_minimum_height() {
        let mut w = build::<App, _>(&trigger_view(Vec::new()));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 400.0)));
        assert_eq!(size, Size::new(300.0, TRIGGER_MIN_HEIGHT));
        assert!(w.chips.is_empty());
        assert!(w.field.size().width >= INPUT_MIN_WIDTH);
    }

    /// A first build seeds the app's existing selection at rest, so mounting a
    /// pre-filled control does not replay an entrance nobody asked for.
    #[test]
    fn a_first_build_seeds_its_chips_at_rest_and_a_later_one_pops_in() {
        let mut w = build::<App, _>(&trigger_view(vec![0]));
        assert_eq!(w.chips.len(), 1);
        assert_eq!(w.chips[0].presence.value(), 1.0, "no entrance on mount");

        reselect(&mut w, vec![0], vec![0, 1]);
        assert_eq!(w.chips.len(), 2);
        assert_eq!(w.chips[1].index, 1);
        assert_eq!(w.chips[1].presence.value(), 0.0, "the new chip pops in");
        assert_eq!(w.chips[1].presence.target(), 1.0);
    }

    /// A removed chip leaves the flow at once and wipes where it stood — the
    /// `popLayout` contract.
    #[test]
    fn a_removed_chip_leaves_the_flow_and_wipes_before_it_is_dropped() {
        let mut w = build::<App, _>(&trigger_view(vec![0, 1]));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        let leaving_rect = w.chips[0].rect;
        assert!(leaving_rect.width() > 0.0);

        reselect(&mut w, vec![0, 1], vec![1]);
        assert_eq!(w.chips.len(), 2, "the leaving chip is kept mounted");
        assert!(w.chips[0].leaving);
        assert_eq!(w.live().count(), 1, "but it no longer takes layout space");

        // Mid-wipe it is clipped from its leading edge, and it still paints.
        paint_at(&mut w, size, 0.0);
        let rec = paint_at(&mut w, size, 60.0);
        let (clip_origin, clip_size) = rec.clips[0];
        assert!(
            clip_origin.x > leaving_rect.x0,
            "the wipe eats the leading edge: {clip_origin:?}"
        );
        assert!(clip_size.width < leaving_rect.width());

        // The surviving chip has already closed the gap.
        let after = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 400.0)));
        assert_eq!(after.height, TRIGGER_MIN_HEIGHT);
        assert_eq!(w.chips[1].rect.x0, TRIGGER_PADDING_X);

        // Once the wipe finishes the cell is dropped by the next rebuild.
        paint_at(&mut w, size, 5_000.0);
        reselect(&mut w, vec![1], vec![1]);
        assert_eq!(w.chips.len(), 1);
        assert_eq!(w.chips[0].index, 1);
    }

    /// Re-selecting a value mid-exit puts the same chip back rather than leaving
    /// a ghost beside a fresh copy.
    #[test]
    fn re_selecting_a_leaving_chip_puts_it_back_in_the_flow() {
        let mut w = build::<App, _>(&trigger_view(vec![0]));
        reselect(&mut w, vec![0], Vec::new());
        assert!(w.chips[0].leaving);
        reselect(&mut w, Vec::new(), vec![0]);
        assert_eq!(w.chips.len(), 1, "no second copy");
        assert!(!w.chips[0].leaving);
        assert_eq!(w.chips[0].presence.target(), 1.0);
    }

    #[test]
    fn chips_wrap_onto_a_second_row_when_the_field_runs_out() {
        let many: Vec<MultiSelectOption> = (0..6)
            .map(|i| multi_select_option(format!("option {i}")))
            .collect();
        let anchor = OverlayAnchor::new();
        let view = multi_select_trigger::<App, _>(
            &anchor,
            many,
            (0..6).collect(),
            "",
            |_: &mut App, _| {},
        );
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(220.0, 600.0)));
        assert!(
            size.height > TRIGGER_MIN_HEIGHT,
            "six chips do not fit on one row of 220px: {size:?}"
        );
        let rows: Vec<f64> = w.chips.iter().map(|chip| chip.rect.y0).collect();
        assert!(
            rows.windows(2).any(|pair| pair[1] > pair[0]),
            "some chip started a new row: {rows:?}"
        );
    }

    #[test]
    fn a_release_on_a_chips_remove_button_reports_that_options_index() {
        let mut w = build::<App, _>(&trigger_view(vec![1]));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 400.0)));
        let mut app = App::default();
        let cross = ChipCell::remove_rect(w.chips[0].rect).center();

        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, cross.x, cross.y),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, cross.x, cross.y),
        );
        assert_eq!(app.removed, vec![1], "the option index, not the chip slot");
        assert!(app.opens.is_empty(), "a remove press never opens the panel");
    }

    #[test]
    fn a_press_on_the_field_asks_to_open_and_an_open_field_does_not() {
        let mut w = build::<App, _>(&trigger_view(Vec::new()));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 400.0)));
        let mut app = App::default();
        // Below the search slot, inside the field: nothing else claims it.
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 290.0, 40.0),
        );
        assert_eq!(app.opens, vec![true]);

        w.open = true;
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, 290.0, 40.0),
        );
        assert_eq!(app.opens, vec![true], "opening is reported once");
    }

    #[test]
    fn a_disabled_field_removes_nothing_and_opens_nothing() {
        let anchor = OverlayAnchor::new();
        let view =
            multi_select_trigger::<App, _>(&anchor, options(), vec![0], "", |_: &mut App, _| {})
                .disabled(true)
                .on_remove(|s: &mut App, i| s.removed.push(i))
                .on_open_change(|s: &mut App, open| s.opens.push(open));
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 400.0)));
        let mut app = App::default();
        let cross = ChipCell::remove_rect(w.chips[0].rect).center();
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Down, cross.x, cross.y),
        );
        dispatch(
            &mut w,
            &mut app,
            size,
            &pointer(PointerPhase::Up, cross.x, cross.y),
        );
        assert!(app.removed.is_empty());
        assert!(app.opens.is_empty());
    }

    #[test]
    fn the_field_captures_its_window_rect_into_the_anchor() {
        let anchor = OverlayAnchor::new();
        let view =
            multi_select_trigger::<App, _>(&anchor, options(), vec![0], "", |_: &mut App, _| {});
        let mut w = build::<App, _>(&view);
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(280.0, 400.0)));
        paint_at(&mut w, size, 0.0);
        assert_eq!(anchor.rect(), Rect::from_origin_size(Point::ZERO, size));
    }

    #[test]
    fn reduce_motion_lands_every_chip_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut w = build::<App, _>(&trigger_view(vec![0]));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(300.0, 400.0)));
        reselect(&mut w, vec![0], vec![0, 1]);
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut Recorder::default());
        assert_eq!(w.chips[1].presence.value(), 1.0, "landed on the spot");
        assert!(!ctx.needs_frame());
    }

    // ---- The panel --------------------------------------------------------

    #[test]
    fn a_row_press_toggles_rather_than_commits() {
        let mut w = build::<App, _>(&panel_view(vec![0], ""));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        let mut app = App::default();

        // Already checked: the press reports it again, and the app decides.
        let row = w.row_rect(0).expect("row 0").center();
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
        assert_eq!(app.toggled, vec![0, 1]);

        // The disabled row is not a hit at all.
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
        assert_eq!(app.toggled, vec![0, 1]);
    }

    #[test]
    fn every_checked_row_draws_a_check() {
        let mut w = build::<App, _>(&panel_view(vec![0, 1], ""));
        let size = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert!(w.is_checked(0) && w.is_checked(1) && !w.is_checked(2));
        // A paint smoke pass: the panel fills its surface and every row.
        let rec = paint_at(&mut w, size, 0.0);
        assert!(!rec.rrects.is_empty());
    }

    #[test]
    fn the_panel_filters_live_and_collapses_what_the_query_drops() {
        let mut w = build::<App, _>(&panel_view(Vec::new(), ""));
        let all = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert_eq!(
            all.height,
            combobox::LIST_PADDING * 2.0 + 3.0 * combobox::ITEM_HEIGHT
        );

        let prev = panel_view(Vec::new(), "");
        let next = panel_view(Vec::new(), "gem");
        let mut counter = 0u64;
        View::<App>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        paint_at(&mut w, all, 0.0);
        paint_at(&mut w, all, 5_000.0);
        let one = layout(&mut w, &BoxConstraints::loose(Size::new(400.0, 600.0)));
        assert_eq!(
            one.height,
            combobox::LIST_PADDING * 2.0 + combobox::ITEM_HEIGHT,
            "only the keyword match survives"
        );
    }

    // ---- The mounted pair -------------------------------------------------

    #[test]
    fn the_public_builders_construct_and_dismiss_through_the_overlay_seam() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(20.0, 20.0, 240.0, 64.0));
        let view = multi_select::<App, _>(options(), vec![0], "", |s: &mut App, i| {
            s.toggled.push(i);
        })
        .anchor(&anchor)
        .side(OverlaySide::Bottom)
        .align(OverlayAlign::Start)
        .open(true)
        .on_open_change(|s: &mut App, open| s.opens.push(open));
        assert_eq!(view.placement.offset, combobox::PANEL_OFFSET);

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
        let _ = app.selected;
    }

    // ---- Typeface: chips, rows and the empty state follow the live theme ---

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(400.0, 600.0);

    /// An open panel filtered by `query`, mounted directly.
    fn probe_panel(query: &str) -> MultiSelectPanelView<()> {
        MultiSelectPanelView {
            options: options(),
            selected: vec![0],
            query: query.to_string(),
            config: Rc::new(RefCell::new(PanelConfig {
                open: true,
                anchor: None,
            })),
            on_toggle: Rc::new(|_: &mut (), _| {}),
        }
    }

    /// A token field holding two chips, over an unfiltered panel (its rows)
    /// and one whose query keeps nothing (its empty state). The token field's
    /// search slot is empty and, with a selection, shows no placeholder: its
    /// baseline field has no themed-family opt-in, so its text would paint the
    /// system face and is not what these tests pin.
    fn probe_view(_: &mut ()) -> frust::FlexView<()> {
        frust::column()
            .child(multi_select_trigger::<(), _>(
                &OverlayAnchor::new(),
                options(),
                vec![0, 1],
                "",
                |_: &mut (), _| {},
            ))
            .child(probe_panel(""))
            .child(probe_panel("zzz"))
    }

    #[test]
    fn chips_and_panel_text_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the multi-select's text", probe_view, PROBE_WINDOW);
    }

    #[test]
    fn chips_and_panel_text_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the multi-select's text", probe_view, PROBE_WINDOW);
    }
}
