// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/dropdown_menus/components/` — `m3e_dropdown_menu_field.dart`
// (the field box, its trailing slot and its overlay alphas),
// `m3e_dropdown_chips.dart` (`M3ESpringChip`'s scale ramp and body) and
// `m3e_dropdown_menu_chip_build.dart` (the wrapped chip strip and the "+N"
// overflow indicator), plus `m3e_dropdown_menu_actions.dart`'s `_toggle`,
// retrieved 2026-08-20.
// Upstream: <https://github.com/paadevelopments/material_3_expressive>
// Porting decisions: the reference's `InkWell` splash and its per-state
// `hoverRadius`/`pressedRadius`/`selectedBorderRadius` morph are **not**
// ported — all three default to `null`, so a stock field never morphs, and the
// catalog has no ink-splash primitive; chips are painted here rather than
// mounted as `crate::input_chip` pods, and their slide/squish micro-animations
// are dropped (see `super`'s chip divergence).

//! The dropdown's trigger: a text-field-shaped tappable box carrying the hint,
//! the selected label (single-select) or the selected chips (multi-select), and
//! a trailing slot holding the rotating arrow, the clear affordance, or a busy
//! indicator.
//!
//! # It is its own anchor
//!
//! The field writes `Rect::from_origin_size(ctx.origin(), ctx.size())` into the
//! [`OverlayAnchor`] it was handed on every paint — the same one-line
//! window-space capture [`crate::overlay::overlay_anchor`] performs, inlined so
//! a dropdown's trigger needs no wrapper. The panel reads the same cell for
//! both its placement and its width.
//!
//! # Not a text field
//!
//! The reference's trigger is a `Material` + `InkWell`, never an editable
//! (`m3e_dropdown_menu_field.dart:251`): its "search" mode puts a `TextField`
//! **inside the panel** instead (`m3e_dropdown_menu_panel.dart:203`). This port
//! keeps that shape, so the field here shares
//! [`mod@crate::text_field`]'s *silhouette* — a 56dp-ish container with a
//! trailing slot — without wrapping an editable or the framework's IME session.
//!
//! # The arrow is the one ported spring
//!
//! `_arrowCtrl.animateTo(math.pi)` on open and `animateTo(0)` on close
//! (`m3e_dropdown_menu_actions.dart:21`), on the reference's
//! `M3EMotion.expressiveSpatialDefault` spring: the glyph rotates a half turn
//! about its own center. Everything else the reference springs belongs to the
//! panel, which the anchored host now ramps
//! ([`super`]'s *The host owns the ramp*).
//!
//! # Chips grow the field
//!
//! In multi-select the field renders one chip per selected option, wrapped into
//! as many rows as they need (`chipStyle.wrap`, the reference's default), and
//! the field's own height follows — the growth `AnimatedSize` smooths upstream
//! (`m3e_dropdown_menu_lifecycle.dart:174`) and this port takes as a plain
//! relayout. Each chip springs in on arrival and springs **out** on removal:
//! a chip dropped from the app's selection is kept in the retained strip, its
//! scale driven to zero, and only then released, so a controlled removal still
//! plays its exit.
//!
//! A chip's delete glyph owns its own hit region and reports the selection
//! *without* that value; the rest of the chip is inert and falls through to the
//! field's own press, which is what the reference's
//! `GestureDetector`-inside-`MouseRegion` does.

use frust::authoring::text::TextStyle;
use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, CursorIcon,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_children, route_event_single, teardown_child, visit_children,
};
use frust::{AnimationController, Theme};

use super::{
    DROPDOWN_CHIP_H_PADDING, DROPDOWN_CHIP_ICON_SIZE, DROPDOWN_CHIP_LABEL_GAP,
    DROPDOWN_CHIP_RADIUS, DROPDOWN_CHIP_RUN_SPACING, DROPDOWN_CHIP_SPACING,
    DROPDOWN_CHIP_V_PADDING, DROPDOWN_FIELD_H_PADDING, DROPDOWN_FIELD_HOVER_ALPHA,
    DROPDOWN_FIELD_LOADING_SIZE, DROPDOWN_FIELD_LOADING_STROKE, DROPDOWN_FIELD_PRESSED_ALPHA,
    DROPDOWN_FIELD_V_PADDING, DROPDOWN_FLING_VELOCITY, DROPDOWN_HINT, DROPDOWN_ICON_GAP,
    DROPDOWN_ICON_SIZE, DROPDOWN_PROGRESS_EPSILON, DROPDOWN_SPRING, DropdownColors, DropdownItem,
    Glyph, OnOpen, OnSelection, Run, SHAPING_INK, container_radius, ordered_selection,
};
use crate::icons;
use crate::overlay::{OverlayAnchor, with_alpha};
use crate::press::presses;
use crate::progress::{ProgressValue, circular_progress};

/// Width used when the incoming constraints are horizontally unbounded — the
/// same fallback [`mod@crate::text_field`] applies for the same reason.
const UNBOUNDED_WIDTH: f64 = 200.0;

/// The half turn the arrow travels between closed and open
/// (`_arrowCtrl.animateTo(math.pi)`), in radians.
const ARROW_TURN: f64 = std::f64::consts::PI;

/// What the field's trailing slot holds, resolved in the reference's own
/// precedence order (`_buildFieldTrailing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trailing {
    /// The busy indicator, while `loading` is set.
    Busy,
    /// The clear-all affordance, when `show_clear` is on and something is
    /// selected.
    Clear,
    /// The rotating chevron.
    Arrow,
}

/// One selected option's chip: its label run, its spring, and the box the last
/// layout placed it in.
struct Chip {
    /// The option's value — the identity a delete reports against.
    value: String,
    label: Run,
    /// The scale ramp: `1.0` fully present, `0.0` gone.
    scale: AnimationController,
    /// The progress the last paint resolved.
    progress: f64,
    /// Whether this chip is on its way out and should be released once its
    /// ramp settles.
    removing: bool,
    /// Whether the ramp has been released yet (the entrance is seeded on the
    /// first paint, so a chip mounted with the field starts from zero).
    started: bool,
    /// The chip's placed box, in the field's own space.
    rect: Rect,
    /// The delete glyph's own hit region, inside [`Self::rect`].
    delete: Rect,
}

impl Chip {
    /// A chip for `value` labelled `label`, unshaped and unstarted.
    fn new(value: &str, label: &str) -> Self {
        Chip {
            value: value.to_string(),
            label: Run::new(label),
            scale: AnimationController::new(std::time::Duration::ZERO),
            progress: 0.0,
            removing: false,
            started: false,
            rect: Rect::ZERO,
            delete: Rect::ZERO,
        }
    }

    /// The scale the chip is drawn at, clamped out of the spring's overshoot
    /// for layout purposes but left free for paint.
    fn layout_scale(&self) -> f64 {
        self.progress.clamp(0.0, 1.0)
    }
}

/// A declarative dropdown trigger. See [`dropdown_field`].
pub struct DropdownFieldView<State: 'static> {
    items: Vec<DropdownItem>,
    selected: Vec<String>,
    hint: String,
    multi: bool,
    open: bool,
    enabled: bool,
    loading: bool,
    error: bool,
    show_clear: bool,
    max_chips: Option<usize>,
    anchor: OverlayAnchor,
    on_open: Option<OnOpen<State>>,
    on_change: Option<OnSelection<State>>,
}

/// Build a dropdown trigger over `items`.
///
/// Hand it the same [`OverlayAnchor`] the [`dropdown`](super::dropdown) half
/// uses ([`DropdownFieldView::anchor`]); without one the field still renders,
/// it just anchors nothing.
///
/// Controlled throughout: a press reports the open state it *requests* through
/// [`on_open`](DropdownFieldView::on_open), and a chip delete or a clear-all
/// reports the selection it requests through
/// [`on_change`](DropdownFieldView::on_change).
pub fn dropdown_field<State: 'static>(items: Vec<DropdownItem>) -> DropdownFieldView<State> {
    DropdownFieldView {
        items,
        selected: Vec::new(),
        hint: DROPDOWN_HINT.to_string(),
        multi: false,
        open: false,
        enabled: true,
        loading: false,
        error: false,
        show_clear: false,
        max_chips: None,
        anchor: OverlayAnchor::new(),
        on_open: None,
        on_change: None,
    }
}

impl<State: 'static> DropdownFieldView<State> {
    /// Capture this field's window-space rect into `anchor` on every paint.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.anchor = anchor.clone();
        self
    }

    /// Hand the field the app-confirmed selection (option values).
    pub fn selected(mut self, selected: Vec<String>) -> Self {
        self.selected = selected;
        self
    }

    /// Replace the placeholder shown when nothing is selected (`hintText`).
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    /// Render the selection as chips rather than as a single label — the
    /// multi-select presentation (`showChipAnimation` over a multi-select
    /// controller).
    pub fn multi(mut self, multi: bool) -> Self {
        self.multi = multi;
        self
    }

    /// Tell the field whether its panel is open, which is what points the
    /// arrow up.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set whether the field accepts a press (default `true`). A disabled field
    /// dims its content, refuses to open, and drops its chips' delete
    /// affordances — the reference's `enabled` gate.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Swap the trailing slot for a busy indicator and refuse to open — the
    /// field half of the async poll seam (see [`super`]).
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Outline the container in the `error` role. The reference recolors its
    /// border from a `FormField`'s `hasError`; with that wrapper descoped, this
    /// is the app-driven equivalent (see [`super`]'s descope note).
    pub fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }

    /// Show a clear-all affordance in the trailing slot whenever something is
    /// selected (`showClearIcon`, off by default). It replaces the arrow while
    /// visible, exactly as upstream.
    pub fn show_clear(mut self, show_clear: bool) -> Self {
        self.show_clear = show_clear;
        self
    }

    /// Cap the number of chips rendered, past which a `+N` indicator stands in
    /// for the rest (`chipStyle.maxDisplayCount`; unlimited by default).
    pub fn max_chips(mut self, max: usize) -> Self {
        self.max_chips = Some(max);
        self
    }

    /// Set the open callback: a press reports `!open`, the reference's
    /// `_toggle`.
    pub fn on_open<F: Fn(&mut State, bool) + 'static>(mut self, on_open: F) -> Self {
        self.on_open = Some(std::rc::Rc::new(on_open));
        self
    }

    /// Set the selection callback: a chip delete reports the selection without
    /// that value, and the clear affordance reports an empty one.
    pub fn on_change<F: Fn(&mut State, Vec<String>) + 'static>(mut self, on_change: F) -> Self {
        self.on_change = Some(std::rc::Rc::new(on_change));
        self
    }

    /// The values whose chips are rendered, and how many are hidden behind the
    /// `+N` indicator.
    fn displayed(&self) -> (Vec<String>, usize) {
        let ordered = ordered_selection(&self.items, &self.selected);
        match self.max_chips {
            Some(max) if ordered.len() > max => {
                let hidden = ordered.len() - max;
                (ordered.into_iter().take(max).collect(), hidden)
            }
            _ => (ordered, 0),
        }
    }

    /// The label `value` names, or the value itself when no item claims it.
    fn label_of(&self, value: &str) -> String {
        self.items
            .iter()
            .find(|item| item.value() == value)
            .map_or_else(|| value.to_string(), |item| item.label().to_string())
    }

    /// The busy indicator's view, in a zero-or-one list so a `loading` flip
    /// reconciles as an ordinary child-list length change.
    fn busy_views(&self) -> Vec<AnyView<State>> {
        if !self.loading {
            return Vec::new();
        }
        vec![any(circular_progress(ProgressValue::Indeterminate)
            .diameter(DROPDOWN_FIELD_LOADING_SIZE)
            .stroke_width(DROPDOWN_FIELD_LOADING_STROKE))]
    }

    /// The single-select label a field shows, or `None` when the chips or the
    /// hint take the content slot instead (`_buildFieldContent`'s ladder).
    fn single_label(&self, ordered: &[String]) -> Option<String> {
        if self.multi || ordered.is_empty() {
            return None;
        }
        Some(self.label_of(&ordered[0]))
    }

    /// The trailing slot's content, in the reference's precedence order.
    fn trailing(&self) -> Trailing {
        if self.loading {
            Trailing::Busy
        } else if self.show_clear && self.enabled && !self.selected.is_empty() {
            Trailing::Clear
        } else {
            Trailing::Arrow
        }
    }
}

/// The retained widget for a [`DropdownFieldView`].
pub struct DropdownFieldWidget {
    hint: Run,
    /// The single-select label, present only when exactly one value is
    /// selected and `multi` is off.
    value_label: Option<Run>,
    chips: Vec<Chip>,
    /// The `+N` overflow indicator's run and count, when chips are capped.
    overflow: Option<(Run, usize)>,
    arrow: Glyph,
    clear: Glyph,
    delete: Glyph,
    trailing: Trailing,
    /// The busy indicator's pod, zero or one.
    busy: Vec<ChildPod>,
    multi: bool,
    open: bool,
    enabled: bool,
    error: bool,
    loading: bool,
    /// Every selected value, in item order — what a delete subtracts from.
    selected: Vec<String>,
    anchor: OverlayAnchor,
    on_open: Option<ErasedArgCallback<bool>>,
    on_change: Option<ErasedArgCallback<Vec<String>>>,
    /// The arrow's rotation ramp.
    arrow_anim: AnimationController,
    arrow_progress: f64,
    arrow_started: bool,
    /// The latched hover flag (the catalog's three-part hover seam).
    hovered: bool,
    pressed: bool,
    press_inside: bool,
    captured: bool,
    /// Which chip's delete glyph a live press is armed on, if any.
    armed_delete: Option<usize>,
    /// Whether a live press is armed on the trailing clear affordance.
    armed_clear: bool,
    width: f64,
    height: f64,
    /// The trailing slot's box, in the field's own space.
    trailing_rect: Rect,
    /// The `+N` indicator's box, in the field's own space.
    overflow_rect: Rect,
}

impl DropdownFieldWidget {
    /// The field's resolved width, in logical px.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// The field's resolved height, in logical px — which grows with the chip
    /// strip.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// The arrow's rotation, in radians: `0` closed, π open.
    pub fn arrow_radians(&self) -> f64 {
        self.arrow_progress * ARROW_TURN
    }

    /// The number of chips currently retained, including any still playing
    /// their exit ramp.
    pub fn chip_count(&self) -> usize {
        self.chips.len()
    }

    /// Reconcile the retained chip strip against `next`.
    ///
    /// Three moves, in order: a value that vanished is **marked removing** and
    /// its ramp reversed; a value that came back mid-exit is **revived**; and
    /// the survivors are re-ordered into `next`'s order — which is item order,
    /// the only order the reference ever renders — with every still-fading chip
    /// slotted back at the index it already held, so a removal plays out where
    /// it stood rather than jumping to the end.
    fn sync_chips(&mut self, next: &[String], labels: &[String]) -> bool {
        let mut changed = false;
        for chip in self.chips.iter_mut() {
            let present = next.iter().any(|v| v == &chip.value);
            if present && chip.removing {
                chip.removing = false;
                chip.scale.fling(DROPDOWN_FLING_VELOCITY, DROPDOWN_SPRING);
                changed = true;
            } else if !present && !chip.removing {
                chip.removing = true;
                chip.scale.fling(-DROPDOWN_FLING_VELOCITY, DROPDOWN_SPRING);
                changed = true;
            }
        }
        let mut fading: Vec<(usize, Chip)> = Vec::new();
        let mut present: Vec<Chip> = Vec::new();
        for (i, chip) in std::mem::take(&mut self.chips).into_iter().enumerate() {
            if chip.removing {
                fading.push((i, chip));
            } else {
                present.push(chip);
            }
        }
        let mut ordered: Vec<Chip> = Vec::with_capacity(next.len() + fading.len());
        for (value, label) in next.iter().zip(labels.iter()) {
            match present.iter().position(|chip| &chip.value == value) {
                Some(i) => ordered.push(present.remove(i)),
                None => {
                    ordered.push(Chip::new(value, label));
                    changed = true;
                }
            }
        }
        // Nothing should be left — the marking pass above turns every absent
        // chip into a fading one — but keep any straggler rather than dropping
        // retained state on the floor.
        ordered.append(&mut present);
        for (index, chip) in fading {
            let at = index.min(ordered.len());
            ordered.insert(at, chip);
        }
        self.chips = ordered;
        changed
    }

    /// Advance every chip's ramp and the arrow's, releasing a settled exit.
    /// Returns whether another frame is needed.
    ///
    /// A live chip ramp also requests a **relayout**, not just a repaint: a
    /// chip's scale drives its own advance in the strip (see
    /// [`Chip::layout_scale`]), so its box and its delete region are only
    /// truthful for the frame the layout that produced them ran in. Gating
    /// that request solely on `advance`'s own return would miss its
    /// *landing* frame — `AnimationController::advance` reports `false` on
    /// the very pass that snaps the value onto its target — so each chip's
    /// progress is compared before/after advancing, the same check the
    /// reduce-motion arm already runs for its snap.
    fn advance(&mut self, ctx: &mut PaintCtx, reduce: bool) -> bool {
        let mut animating = false;
        let now = ctx.frame_time();

        if !self.arrow_started {
            self.arrow_started = true;
            self.arrow_progress = if self.open { 1.0 } else { 0.0 };
        } else if reduce {
            if self.arrow_anim.is_animating() {
                self.arrow_anim.stop();
            }
            self.arrow_progress = if self.open { 1.0 } else { 0.0 };
        } else if self.arrow_anim.is_animating() {
            if self.arrow_anim.advance(now) {
                animating = true;
            }
            self.arrow_progress = self.arrow_anim.value_clamped();
        }

        let mut settled: Vec<usize> = Vec::new();
        let mut chips_moved = false;
        for (i, chip) in self.chips.iter_mut().enumerate() {
            let progress_before = chip.progress;
            if !chip.started {
                chip.started = true;
                if reduce {
                    chip.progress = 1.0;
                } else {
                    chip.scale.fling(DROPDOWN_FLING_VELOCITY, DROPDOWN_SPRING);
                }
            }
            if reduce {
                if chip.scale.is_animating() {
                    chip.scale.stop();
                }
                chip.progress = if chip.removing { 0.0 } else { 1.0 };
                // The reduce-motion snap is still a layout-relevant value
                // change (`layout_scale` feeds the chip's box/delete-hit-box
                // sizing) — most commonly a freshly-mounted chip going
                // 0.0 → 1.0, since `layout` already ran this frame against
                // the pre-snap value. Request the same relayout the animating
                // arm below requests for an in-flight ramp.
                if chip.progress != progress_before {
                    chips_moved = true;
                }
            } else if chip.scale.is_animating() {
                if chip.scale.advance(now) {
                    animating = true;
                }
                chip.progress = chip.scale.value();
                // Comparing before/after (not just `advance`'s own return)
                // is what actually catches the ramp's *landing* frame:
                // `AnimationController::advance` reports `false` on the
                // very pass that snaps the value onto its target, so
                // gating solely on it would leave the last chip box/
                // delete-hit-box `layout` ever saw a hair short of rest —
                // the same trap the reduce-motion arm above already
                // guards against.
                if chip.progress != progress_before {
                    chips_moved = true;
                }
            }
            if chip.removing && chip.progress <= DROPDOWN_PROGRESS_EPSILON {
                settled.push(i);
            }
        }
        for i in settled.into_iter().rev() {
            self.chips.remove(i);
            chips_moved = true;
        }
        if chips_moved {
            ctx.request_layout();
        }
        animating
    }

    /// Whether the content slot holds the chip strip rather than a label or
    /// the hint — true for a multi-select with anything to show, including a
    /// lone `+N` indicator.
    fn has_chip_strip(&self) -> bool {
        self.multi && (!self.chips.is_empty() || self.overflow.is_some())
    }

    /// Report the selection a delete of `value` requests.
    fn delete_chip(&mut self, ctx: &mut EventCtx, value: &str) {
        let Some(on_change) = self.on_change.as_mut() else {
            return;
        };
        let next: Vec<String> = self
            .selected
            .iter()
            .filter(|v| v.as_str() != value)
            .cloned()
            .collect();
        on_change(ctx, next);
    }

    /// The chip whose delete glyph covers `pos`, if any.
    fn delete_at(&self, pos: Point) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        self.chips
            .iter()
            .position(|chip| !chip.removing && chip.delete.contains(pos))
    }

    /// The `Widget::event` pointer arm.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let inside =
            |pos: Point| pos.x >= 0.0 && pos.x < self.width && pos.y >= 0.0 && pos.y < self.height;
        match p.phase {
            PointerPhase::Move if !self.captured => {
                let over = inside(p.position);
                if over {
                    ctx.claim_hover();
                    if self.enabled {
                        ctx.set_cursor(CursorIcon::Pointer);
                    }
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Move => {
                let still = inside(p.position);
                if self.press_inside != still {
                    self.press_inside = still;
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position) {
                    return EventResult::Ignored;
                }
                // A chip's delete glyph and the clear affordance each own their
                // own region; everything else is the field's own press.
                self.armed_delete = self.delete_at(p.position);
                self.armed_clear = self.armed_delete.is_none()
                    && self.trailing == Trailing::Clear
                    && self.trailing_rect.contains(p.position);
                self.pressed = true;
                self.press_inside = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let released_inside = inside(p.position);
                let armed_delete = self.armed_delete;
                let armed_clear = self.armed_clear;
                self.pressed = false;
                self.press_inside = false;
                self.captured = false;
                self.armed_delete = None;
                self.armed_clear = false;
                ctx.request_redraw();
                if !released_inside {
                    return EventResult::Handled;
                }
                if let Some(i) = armed_delete {
                    if self.delete_at(p.position) == Some(i) {
                        let value = self.chips[i].value.clone();
                        self.delete_chip(ctx, &value);
                    }
                    return EventResult::Handled;
                }
                if armed_clear {
                    if self.trailing_rect.contains(p.position)
                        && let Some(on_change) = self.on_change.as_mut()
                    {
                        on_change(ctx, Vec::new());
                    }
                    return EventResult::Handled;
                }
                // `_toggle`: a disabled or loading field opens nothing.
                if self.enabled
                    && !self.loading
                    && let Some(on_open) = self.on_open.as_mut()
                {
                    let open = self.open;
                    on_open(ctx, !open);
                }
                EventResult::Handled
            }
            // A `Cancel` arm touches no app state — only the flags the next
            // move re-establishes.
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.press_inside = false;
                self.captured = false;
                self.armed_delete = None;
                self.armed_clear = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

impl<State: 'static> View<State> for DropdownFieldView<State> {
    type Element = DropdownFieldWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DropdownFieldWidget {
        let (displayed, hidden) = self.displayed();
        let labels: Vec<String> = displayed.iter().map(|v| self.label_of(v)).collect();
        let chips = if self.multi {
            displayed
                .iter()
                .zip(labels.iter())
                .map(|(value, label)| Chip::new(value, label))
                .collect()
        } else {
            Vec::new()
        };
        let ordered = ordered_selection(&self.items, &self.selected);
        DropdownFieldWidget {
            hint: Run::new(self.hint.clone()),
            value_label: self.single_label(&ordered).map(Run::new),
            chips,
            overflow: (self.multi && hidden > 0).then(|| (Run::new(format!("+{hidden}")), hidden)),
            arrow: Glyph::new(icons::KEYBOARD_ARROW_DOWN, DROPDOWN_ICON_SIZE),
            // The reference's clear affordance is `Icons.clear`, the same glyph
            // this catalog generated under the `CLOSE` name.
            clear: Glyph::new(icons::CLOSE, DROPDOWN_ICON_SIZE),
            delete: Glyph::new(icons::CLOSE, DROPDOWN_CHIP_ICON_SIZE),
            trailing: self.trailing(),
            busy: self
                .busy_views()
                .iter()
                .map(|view| build_child(view, ctx))
                .collect(),
            multi: self.multi,
            open: self.open,
            enabled: self.enabled,
            error: self.error,
            loading: self.loading,
            selected: ordered,
            anchor: self.anchor.clone(),
            on_open: self.on_open.as_ref().map(erase_callback_arg),
            on_change: self.on_change.as_ref().map(erase_callback_arg),
            arrow_anim: AnimationController::new(std::time::Duration::ZERO),
            arrow_progress: 0.0,
            arrow_started: false,
            hovered: false,
            pressed: false,
            press_inside: false,
            captured: false,
            armed_delete: None,
            armed_clear: false,
            width: 0.0,
            height: 0.0,
            trailing_rect: Rect::ZERO,
            overflow_rect: Rect::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DropdownFieldWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_children(
            &prev.busy_views(),
            &self.busy_views(),
            &mut element.busy,
            ctx,
            |view: &AnyView<State>| view,
            |_| None,
        );
        if prev.hint != self.hint {
            element.hint = Run::new(self.hint.clone());
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let ordered = ordered_selection(&self.items, &self.selected);
        let (displayed, hidden) = self.displayed();
        let labels: Vec<String> = displayed.iter().map(|v| self.label_of(v)).collect();
        if element.selected != ordered || prev.items != self.items || element.multi != self.multi {
            element.selected = ordered.clone();
            element.multi = self.multi;
            element.value_label = self.single_label(&ordered).map(Run::new);
            if self.multi {
                if element.sync_chips(&displayed, &labels) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            } else if !element.chips.is_empty() {
                element.chips.clear();
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        let overflow = (self.multi && hidden > 0).then_some(hidden);
        if element.overflow.as_ref().map(|(_, n)| *n) != overflow {
            element.overflow = overflow.map(|n| (Run::new(format!("+{n}")), n));
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.open != self.open {
            element.open = self.open;
            element.arrow_anim.fling(
                if self.open {
                    DROPDOWN_FLING_VELOCITY
                } else {
                    -DROPDOWN_FLING_VELOCITY
                },
                DROPDOWN_SPRING,
            );
            flags |= ChangeFlags::PAINT;
        }
        let trailing = self.trailing();
        if element.trailing != trailing {
            element.trailing = trailing;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.enabled != self.enabled
            || element.error != self.error
            || element.loading != self.loading
        {
            element.enabled = self.enabled;
            element.error = self.error;
            element.loading = self.loading;
            flags |= ChangeFlags::PAINT;
        }
        element.anchor = self.anchor.clone();
        // Closures aren't comparable, so the adapters are reinstalled
        // unconditionally — cheap, and what every interactive widget does.
        element.on_open = self.on_open.as_ref().map(erase_callback_arg);
        element.on_change = self.on_change.as_ref().map(erase_callback_arg);
        flags
    }

    fn teardown(&self, element: &mut DropdownFieldWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.busy_views().iter().zip(element.busy.iter_mut()) {
            teardown_child(view, pod, ctx);
        }
    }
}

/// The type styles a field's two text roles resolve to: `bodyLarge` for the
/// hint and the selected label (`m3e_dropdown_menu_theme.dart:101`),
/// `labelMedium` for a chip (`chipLabelStyle`).
struct Styles {
    body: TextStyle,
    chip: TextStyle,
}

impl Styles {
    /// Resolve both roles from `theme`, or from the M3 token literals when no
    /// theme is threaded into the pass.
    fn resolve(theme: Option<&Theme>) -> Self {
        let (mut body, mut chip) = match theme {
            Some(theme) => (
                theme.type_scale.body_large.clone(),
                theme.type_scale.label_medium.clone(),
            ),
            None => {
                let body = TextStyle::new(16.0, SHAPING_INK);
                let mut chip = TextStyle::new(12.0, SHAPING_INK);
                chip.letter_spacing = 0.5;
                (body, chip)
            }
        };
        body.color = SHAPING_INK;
        chip.color = SHAPING_INK;
        Styles { body, chip }
    }
}

impl Widget for DropdownFieldWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let styles = Styles::resolve(Theme::from_layout_ctx(ctx));
        self.width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            UNBOUNDED_WIDTH
        };

        // The trailing slot always reserves a full icon box, whatever it holds,
        // so the content column's width never depends on which slot is showing.
        let slot = DROPDOWN_ICON_GAP + DROPDOWN_ICON_SIZE;
        let content_width = (self.width - 2.0 * DROPDOWN_FIELD_H_PADDING - slot).max(0.0);

        let content_height = if self.has_chip_strip() {
            self.layout_chips(ctx, &styles, content_width)
        } else if let Some(label) = self.value_label.as_mut() {
            label.shape(ctx, &styles.body, content_width).height
        } else {
            self.hint.shape(ctx, &styles.body, content_width).height
        };

        self.height = 2.0 * DROPDOWN_FIELD_V_PADDING + content_height.max(DROPDOWN_ICON_SIZE);
        self.trailing_rect = Rect::from_origin_size(
            Point::new(
                self.width - DROPDOWN_FIELD_H_PADDING - DROPDOWN_ICON_SIZE,
                (self.height - DROPDOWN_ICON_SIZE) / 2.0,
            ),
            Size::new(DROPDOWN_ICON_SIZE, DROPDOWN_ICON_SIZE),
        );

        if let Some(pod) = self.busy.first_mut() {
            let extent = DROPDOWN_FIELD_LOADING_SIZE;
            pod.layout_child(ctx, &BoxConstraints::tight(Size::new(extent, extent)));
            pod.set_origin(Point::new(
                self.width - DROPDOWN_FIELD_H_PADDING - extent,
                (self.height - extent) / 2.0,
            ));
        }

        bc.constrain(Size::new(self.width, self.height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Authoritative hover read (the catalog's three-part hover seam).
        if !ctx.is_hovered() {
            self.hovered = false;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = DropdownColors::resolve(theme);
        let radius = container_radius(theme);
        let reduce = crate::overlay::reduce_motion(theme);
        if self.advance(ctx, reduce) {
            ctx.request_frame();
        }

        // `PaintCtx::origin` is absolute window space — the one read that can
        // answer "where is this trigger on screen", which is what the anchored
        // panel needs.
        let origin = ctx.origin();
        self.anchor.set(Rect::from_origin_size(origin, ctx.size()));

        let size = Size::new(self.width, self.height);
        scene.fill_rounded_rect(origin, size, radius, colors.field_container);
        let ink = if self.error {
            colors.error
        } else {
            colors.field_content
        };
        // The reference's own overlay alphas, resolved off the field's ink
        // rather than through this catalog's shared state-layer table, which
        // uses different numbers.
        let overlay = if self.pressed && self.press_inside {
            DROPDOWN_FIELD_PRESSED_ALPHA
        } else if self.hovered {
            DROPDOWN_FIELD_HOVER_ALPHA
        } else {
            0.0
        };
        if overlay > 0.0 && self.enabled {
            scene.fill_rounded_rect(origin, size, radius, with_alpha(ink, overlay));
        }

        let content_x = origin.x + DROPDOWN_FIELD_H_PADDING;
        if self.has_chip_strip() {
            self.paint_chips(origin, &colors, scene);
        } else if let Some(label) = &self.value_label {
            let height = label.size().height;
            label.paint(
                Point::new(content_x, origin.y + (self.height - height) / 2.0),
                ink,
                scene,
            );
        } else {
            let height = self.hint.size().height;
            self.hint.paint(
                Point::new(content_x, origin.y + (self.height - height) / 2.0),
                colors.hint,
                scene,
            );
        }

        match self.trailing {
            Trailing::Busy => {
                if let Some(pod) = self.busy.first_mut() {
                    pod.paint_child(ctx, scene);
                }
            }
            Trailing::Clear => {
                self.clear
                    .paint(self.trailing_rect.origin() + origin.to_vec2(), ink, scene)
            }
            Trailing::Arrow => self.arrow.paint_rotated(
                self.trailing_rect.origin() + origin.to_vec2(),
                ink,
                self.arrow_radians(),
                scene,
            ),
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in self.busy.iter_mut() {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The busy indicator is decoration: it takes no input, but the routing
        // helper still keeps its capture bookkeeping honest.
        if let Some(pod) = self.busy.first_mut()
            && pod.is_active()
        {
            return route_event_single(pod, ctx, event);
        }
        match event {
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = if let Some(value) = &self.value_label {
            value.content.clone()
        } else {
            self.hint.content.clone()
        };
        let open = self.open;
        let enabled = self.enabled;
        ctx.push_node(Role::ComboBox, |node| {
            node.set_label(label.as_str());
            node.set_expanded(open);
            if enabled {
                node.add_action(Action::Click);
            }
        });
    }

    visit_children!(busy);
}

impl DropdownFieldWidget {
    /// Lay the chip strip out into as many rows as `content_width` needs,
    /// returning the strip's total height (`chipStyle.wrap`'s `Wrap`).
    fn layout_chips(&mut self, ctx: &mut LayoutCtx, styles: &Styles, content_width: f64) -> f64 {
        let chip_width = |label: Size, deletable: bool| {
            let mut width = 2.0 * DROPDOWN_CHIP_H_PADDING + label.width;
            if deletable {
                width += DROPDOWN_CHIP_LABEL_GAP + DROPDOWN_CHIP_ICON_SIZE;
            }
            width
        };
        let deletable = self.enabled;
        // A chip never ellipsizes: the reference's chip label is a plain
        // `Text`, and the strip wraps instead.
        let mut sizes: Vec<Size> = Vec::with_capacity(self.chips.len());
        for chip in self.chips.iter_mut() {
            let natural = chip.label.natural_width(ctx, &styles.chip);
            sizes.push(chip.label.shape(ctx, &styles.chip, natural));
        }
        let overflow_size = self
            .overflow
            .as_mut()
            .map(|(run, _)| {
                let natural = run.natural_width(ctx, &styles.chip);
                run.shape(ctx, &styles.chip, natural)
            })
            .unwrap_or(Size::ZERO);

        let row_height = sizes
            .iter()
            .map(|size| size.height)
            .fold(overflow_size.height, f64::max)
            .max(if deletable {
                DROPDOWN_CHIP_ICON_SIZE
            } else {
                0.0
            })
            + 2.0 * DROPDOWN_CHIP_V_PADDING;

        let mut x = 0.0;
        let mut y = 0.0;
        for (i, chip) in self.chips.iter_mut().enumerate() {
            let width = chip_width(sizes[i], deletable) * chip.layout_scale();
            if x > 0.0 && x + width > content_width {
                x = 0.0;
                y += row_height + DROPDOWN_CHIP_RUN_SPACING;
            }
            chip.rect = Rect::from_origin_size(
                Point::new(DROPDOWN_FIELD_H_PADDING + x, DROPDOWN_FIELD_V_PADDING + y),
                Size::new(width, row_height),
            );
            chip.delete = if deletable && width > 0.0 {
                Rect::from_origin_size(
                    Point::new(
                        chip.rect.x1 - DROPDOWN_CHIP_H_PADDING - DROPDOWN_CHIP_ICON_SIZE,
                        chip.rect.y0 + (row_height - DROPDOWN_CHIP_ICON_SIZE) / 2.0,
                    ),
                    Size::new(DROPDOWN_CHIP_ICON_SIZE, DROPDOWN_CHIP_ICON_SIZE),
                )
            } else {
                Rect::ZERO
            };
            x += width + DROPDOWN_CHIP_SPACING;
        }
        if self.overflow.is_some() {
            let width = 2.0 * DROPDOWN_CHIP_H_PADDING + overflow_size.width;
            if x > 0.0 && x + width > content_width {
                x = 0.0;
                y += row_height + DROPDOWN_CHIP_RUN_SPACING;
            }
            self.overflow_rect = Rect::from_origin_size(
                Point::new(DROPDOWN_FIELD_H_PADDING + x, DROPDOWN_FIELD_V_PADDING + y),
                Size::new(width, row_height),
            );
        } else {
            self.overflow_rect = Rect::ZERO;
        }
        y + row_height
    }

    /// Paint the chip strip: each chip's pill, its label, and its delete glyph,
    /// scaled about its own leading edge by the spring ramp.
    fn paint_chips(&self, origin: Point, colors: &DropdownColors, scene: &mut dyn PaintScene) {
        for chip in self.chips.iter() {
            if chip.progress <= DROPDOWN_PROGRESS_EPSILON {
                continue;
            }
            let rect = chip.rect + origin.to_vec2();
            scene.fill_rounded_rect(
                rect.origin(),
                rect.size(),
                DROPDOWN_CHIP_RADIUS,
                colors.chip_container,
            );
            // `Transform(alignment: Alignment.centerLeft)` — the scale pivots on
            // the chip's own leading edge, so a strip collapses leftwards.
            let pivot = kurbo::Vec2::new(rect.x0, rect.y0 + rect.height() / 2.0);
            let scale = chip.progress.clamp(0.0, 1.0);
            let transform = Affine::translate(pivot)
                * Affine::scale_non_uniform(scale, 1.0)
                * Affine::translate(-pivot);
            let label_size = chip.label.size();
            chip.label.paint_transformed(
                Point::new(
                    rect.x0 + DROPDOWN_CHIP_H_PADDING,
                    rect.y0 + (rect.height() - label_size.height) / 2.0,
                ),
                colors.chip_content,
                transform,
                scene,
            );
            if chip.delete.width() > 0.0 {
                self.delete.paint(
                    chip.delete.origin() + origin.to_vec2(),
                    colors.chip_content,
                    scene,
                );
            }
        }
        if let Some((run, _)) = &self.overflow {
            let size = run.size();
            let rect = self.overflow_rect + origin.to_vec2();
            scene.fill_rounded_rect(
                rect.origin(),
                rect.size(),
                DROPDOWN_CHIP_RADIUS,
                colors.chip_container,
            );
            run.paint(
                Point::new(
                    rect.x0 + DROPDOWN_CHIP_H_PADDING,
                    rect.y0 + (rect.height() - size.height) / 2.0,
                ),
                colors.chip_content,
                scene,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dropdown::dropdown_item;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::{BezPath, Brush, Color, PointerButton, text::TextContext};
    use frust::{Brightness, FrameTime};
    use std::any::Any;

    const AREA: Size = Size::new(320.0, 200.0);

    #[derive(Default)]
    struct AppState {
        opens: Vec<bool>,
        selections: Vec<Vec<String>>,
    }

    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        paths: Vec<Point>,
        runs: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn fill_path(&mut self, origin: Point, _path: &BezPath, _brush: &Brush) {
            self.paths.push(origin);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.runs += 1;
        }
    }

    impl Recorder {
        /// The alphas of every translucent rounded rect — the state-layer
        /// washes, filtered out of the opaque container fills.
        fn washes(&self) -> Vec<f32> {
            self.rrects
                .iter()
                .map(|(_, _, _, color)| color.components[3])
                .filter(|alpha| *alpha < 1.0)
                .collect()
        }
    }

    fn fruit() -> Vec<DropdownItem> {
        vec![
            dropdown_item("Apple", "apple"),
            dropdown_item("Banana", "banana"),
            dropdown_item("Cherry", "cherry"),
        ]
    }

    fn view() -> DropdownFieldView<AppState> {
        dropdown_field(fruit())
            .on_open(|state: &mut AppState, open| state.opens.push(open))
            .on_change(|state: &mut AppState, values| state.selections.push(values))
    }

    fn build(view: &DropdownFieldView<AppState>) -> DropdownFieldWidget {
        let mut counter = 0u64;
        View::<AppState>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut DropdownFieldWidget, area: Size) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(area))
    }

    fn ready(view: &DropdownFieldView<AppState>) -> DropdownFieldWidget {
        let mut w = build(view);
        layout(&mut w, AREA);
        w
    }

    fn rebuild(
        w: &mut DropdownFieldWidget,
        prev: &DropdownFieldView<AppState>,
        next: &DropdownFieldView<AppState>,
    ) {
        let mut counter = 0u64;
        View::<AppState>::rebuild(next, prev, w, &mut BuildCtx::new(&mut counter));
    }

    fn paint_at(w: &mut DropdownFieldWidget, now: FrameTime) -> Recorder {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut rec = Recorder::default();
        let size = Size::new(w.width, w.height);
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, now).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn paint(w: &mut DropdownFieldWidget) -> Recorder {
        paint_at(w, FrameTime::ZERO)
    }

    /// Run the widget's own ramps forward far enough to settle.
    fn settle(w: &mut DropdownFieldWidget) {
        for step in 1..200 {
            paint_at(w, FrameTime::from_nanos(step * 16_000_000));
        }
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(
        w: &mut DropdownFieldWidget,
        state: &mut AppState,
        event: &InputEvent,
    ) -> EventResult {
        let any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(any, Point::ZERO, Size::new(w.width, w.height));
        w.event(&mut ctx, event)
    }

    fn click(w: &mut DropdownFieldWidget, state: &mut AppState, at: Point) {
        dispatch(w, state, &pointer(PointerPhase::Down, at.x, at.y));
        dispatch(w, state, &pointer(PointerPhase::Up, at.x, at.y));
    }

    // ---- the anchor -------------------------------------------------------

    #[test]
    fn the_field_captures_its_own_rect_into_the_anchor_on_paint() {
        let anchor = OverlayAnchor::new();
        let mut w = ready(&view().anchor(&anchor));
        assert_eq!(anchor.rect(), Rect::ZERO, "nothing captured before paint");
        paint(&mut w);
        assert_eq!(
            anchor.rect(),
            Rect::from_origin_size(Point::ORIGIN, Size::new(w.width(), w.height()))
        );
    }

    // ---- content ----------------------------------------------------------

    #[test]
    fn an_empty_selection_shows_the_hint() {
        let mut w = ready(&view());
        assert!(w.value_label.is_none());
        let rec = paint(&mut w);
        assert!(rec.runs > 0);
    }

    #[test]
    fn a_single_selection_shows_its_label_not_its_value() {
        let w = ready(&view().selected(vec!["banana".to_string()]));
        assert_eq!(
            w.value_label.as_ref().map(|run| run.content.as_str()),
            Some("Banana")
        );
    }

    #[test]
    fn a_value_naming_no_item_is_dropped_rather_than_shown_raw() {
        let w = ready(&view().selected(vec!["durian".to_string()]));
        assert!(
            w.value_label.is_none(),
            "an unknown value normalizes away, leaving the hint"
        );
    }

    // ---- the trailing slot ------------------------------------------------

    #[test]
    fn the_trailing_slot_follows_the_reference_s_precedence() {
        assert_eq!(ready(&view()).trailing, Trailing::Arrow);
        assert_eq!(
            ready(&view().show_clear(true).selected(vec!["apple".to_string()])).trailing,
            Trailing::Clear
        );
        assert_eq!(
            ready(
                &view()
                    .show_clear(true)
                    .selected(vec!["apple".to_string()])
                    .loading(true)
            )
            .trailing,
            Trailing::Busy,
            "loading outranks the clear affordance"
        );
        assert_eq!(
            ready(&view().show_clear(true)).trailing,
            Trailing::Arrow,
            "nothing selected, nothing to clear"
        );
        assert_eq!(
            ready(
                &view()
                    .show_clear(true)
                    .enabled(false)
                    .selected(vec!["apple".to_string()])
            )
            .trailing,
            Trailing::Arrow,
            "a disabled field offers no clear affordance"
        );
    }

    #[test]
    fn a_busy_field_mounts_the_indicator_pod() {
        let w = ready(&view().loading(true));
        assert_eq!(w.busy.len(), 1);
        let plain = ready(&view());
        assert_eq!(plain.busy.len(), 0);
    }

    // ---- the arrow --------------------------------------------------------

    #[test]
    fn the_arrow_rests_at_zero_closed_and_a_half_turn_open() {
        let mut closed = ready(&view());
        paint(&mut closed);
        assert_eq!(closed.arrow_radians(), 0.0);

        let mut open = ready(&view().open(true));
        paint(&mut open);
        assert!((open.arrow_radians() - ARROW_TURN).abs() < 1e-9);
    }

    #[test]
    fn opening_springs_the_arrow_round_and_closing_springs_it_back() {
        let prev = view();
        let mut w = ready(&prev);
        paint(&mut w);
        assert_eq!(w.arrow_radians(), 0.0);

        let next = view().open(true);
        rebuild(&mut w, &prev, &next);
        settle(&mut w);
        assert!(
            (w.arrow_radians() - ARROW_TURN).abs() < 1e-3,
            "the arrow settled at a half turn, got {}",
            w.arrow_radians()
        );

        rebuild(&mut w, &next, &prev);
        settle(&mut w);
        assert!(w.arrow_radians().abs() < 1e-3);
    }

    // ---- pressing ---------------------------------------------------------

    #[test]
    fn a_press_requests_the_opposite_open_state_and_never_flips_its_own() {
        let mut w = ready(&view());
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        click(&mut w, &mut state, at);
        assert_eq!(state.opens, vec![true]);
        assert!(!w.open, "controlled: the field waits to be told");
    }

    #[test]
    fn an_open_field_requests_a_close() {
        let mut w = ready(&view().open(true));
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        click(&mut w, &mut state, at);
        assert_eq!(state.opens, vec![false]);
    }

    #[test]
    fn a_disabled_or_loading_field_opens_nothing() {
        for v in [view().enabled(false), view().loading(true)] {
            let mut w = ready(&v);
            let mut state = AppState::default();
            let at = Point::new(40.0, w.height() / 2.0);
            click(&mut w, &mut state, at);
            assert!(state.opens.is_empty());
        }
    }

    #[test]
    fn a_release_outside_the_field_reports_nothing() {
        let mut w = ready(&view());
        let mut state = AppState::default();
        let below = w.height() + 40.0;
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 40.0, 10.0));
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Up, 40.0, below));
        assert!(state.opens.is_empty());
    }

    #[test]
    fn only_a_primary_press_arms_the_field() {
        let mut w = ready(&view());
        let mut state = AppState::default();
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(40.0, w.height() / 2.0),
            button: PointerButton::Secondary,
        });
        assert_eq!(
            dispatch(&mut w, &mut state, &secondary),
            EventResult::Ignored
        );
    }

    #[test]
    fn a_cancel_clears_the_press_without_reporting() {
        let mut w = ready(&view());
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, at.x, at.y));
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Cancel, at.x, at.y),
        );
        assert!(state.opens.is_empty());
        assert!(!w.pressed);
    }

    #[test]
    fn the_clear_affordance_requests_an_empty_selection_instead_of_opening() {
        let mut w = ready(
            &view()
                .show_clear(true)
                .selected(vec!["apple".to_string(), "banana".to_string()]),
        );
        let mut state = AppState::default();
        let at = w.trailing_rect.center();
        click(&mut w, &mut state, at);
        assert_eq!(state.selections, vec![Vec::<String>::new()]);
        assert!(state.opens.is_empty(), "the clear press never toggles");
    }

    // ---- chips ------------------------------------------------------------

    #[test]
    fn a_multi_select_renders_one_chip_per_selected_option() {
        let w = ready(
            &view()
                .multi(true)
                .selected(vec!["apple".to_string(), "cherry".to_string()]),
        );
        assert_eq!(w.chip_count(), 2);
        assert_eq!(w.chips[0].label.content, "Apple");
        assert_eq!(w.chips[1].label.content, "Cherry");
    }

    #[test]
    fn a_single_select_never_renders_a_chip() {
        let w = ready(&view().selected(vec!["apple".to_string()]));
        assert_eq!(w.chip_count(), 0);
    }

    /// Options whose labels are long enough that three chips cannot share one
    /// row in an [`AREA`]-wide field.
    fn long_items() -> Vec<DropdownItem> {
        vec![
            dropdown_item("Passionfruit sorbet", "a"),
            dropdown_item("Blackcurrant cordial", "b"),
            dropdown_item("Clementine marmalade", "c"),
        ]
    }

    #[test]
    fn chips_grow_the_field_past_the_hint_only_row() {
        let plain = ready(&view());
        let field = |selected: Vec<String>| {
            dropdown_field::<AppState>(long_items())
                .multi(true)
                .selected(selected)
        };
        let mut one = ready(&field(vec!["a".to_string()]));
        settle(&mut one);
        layout(&mut one, AREA);
        let mut many = ready(&field(vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
        ]));
        settle(&mut many);
        layout(&mut many, AREA);
        assert!(one.height() >= plain.height() - 1e-6);
        assert!(
            many.height() > one.height(),
            "three long chips wrap past one row in a {}px field ({} vs {})",
            AREA.width,
            many.height(),
            one.height()
        );
    }

    #[test]
    fn a_chip_springs_in_from_nothing() {
        let mut w = ready(&view().multi(true).selected(vec!["apple".to_string()]));
        paint(&mut w);
        assert!(w.chips[0].progress < 1.0, "the entrance starts from zero");
        settle(&mut w);
        assert!((w.chips[0].progress - 1.0).abs() < 1e-2);
    }

    #[test]
    fn a_freshly_added_chip_lands_its_box_exactly_on_the_settle_frame() {
        // The regression guard for the landing-frame trap (gate-r5-05):
        // `AnimationController::advance` reports `false` on the very pass
        // that snaps a chip's scale onto its target, so gating the
        // relayout request solely on that return would leave the last
        // chip box `layout` ever saw a hair short of rest. This drives a
        // shell-honest frame loop — `layout` runs only on a frame the
        // *previous* paint actually asked for one — so a silently-skipped
        // landing frame would leave the chip's box short of its natural
        // width instead of landing on it exactly. `settle()` (used by the
        // sibling test above) can't catch this: it never relayouts and
        // never reads laid-out geometry, only the raw `progress` value,
        // which `paint` always updates regardless of what it requests.
        let prev = view().multi(true);
        let mut w = ready(&prev);
        let next = view().multi(true).selected(vec!["apple".to_string()]);
        rebuild(&mut w, &prev, &next);
        assert_eq!(w.chip_count(), 1);

        let theme = crate::baseline().with_brightness(Brightness::Light);
        // Both signals matter here, unlike the single-value widgets this
        // pattern guards elsewhere: the ramp's zero-delta seeding frame
        // reports `needs_frame` (`animating`, from a truthy `advance`) but
        // not yet `needs_layout` (`chip.progress` hasn't visibly moved),
        // since `chips_moved` — not a bare truthy `advance` — is this
        // widget's only relayout trigger. A shell-honest loop must keep
        // pumping frames on `needs_frame` alone and only relayout on
        // `needs_layout`, exactly as a real shell would.
        let mut needs_layout = true;
        let mut needs_frame = true;
        let mut t = 0u64;
        let mut frames = 0u32;
        let mut last_width = w.chips[0].rect.width();
        loop {
            if !needs_frame {
                break;
            }
            if needs_layout {
                layout(&mut w, AREA);
                last_width = w.chips[0].rect.width();
            }
            let mut rec = Recorder::default();
            let size = Size::new(w.width, w.height);
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(t))
                .with_theme(&theme as &dyn Any);
            w.paint(&mut ctx, &mut rec);
            needs_layout = ctx.needs_layout();
            needs_frame = ctx.needs_frame();
            frames += 1;
            assert!(
                frames < 600,
                "the chip entrance should settle well inside 600 frames"
            );
            t += 16_000_000;
        }
        assert!(frames > 1, "the chip entrance spans more than one frame");

        // Reference: brute-force settle well past the spring's resting
        // time, then one more relayout.
        settle(&mut w);
        layout(&mut w, AREA);
        let target_width = w.chips[0].rect.width();

        assert_eq!(
            last_width, target_width,
            "the last layout a shell-honest driver runs lands exactly on the chip's settled box width"
        );
    }

    #[test]
    fn reduce_motion_gives_a_freshly_added_chip_a_real_box_after_relayout() {
        // Regression guard: a chip added under reduce_motion snaps `progress`
        // straight to `1.0` in `paint`, but `layout_chips` sizes the chip's
        // rect and its delete hit box off `progress` — without a relayout
        // request the new chip keeps the zero-width box `Chip::new` seeds
        // (its label would still paint at full scale, and its delete hit
        // box would stay empty) until some unrelated change happened to
        // trigger one.
        let mut theme = crate::baseline().with_brightness(Brightness::Light);
        theme.motion.reduce_motion = true;

        let prev = view().multi(true);
        let mut w = ready(&prev);
        let next = view().multi(true).selected(vec!["apple".to_string()]);
        rebuild(&mut w, &prev, &next);
        assert_eq!(w.chip_count(), 1);
        assert_eq!(w.chips[0].rect, Rect::ZERO, "not yet laid out");

        let mut rec = Recorder::default();
        let size = Size::new(w.width, w.height);
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::ZERO).with_theme(&theme as &dyn Any);
        w.paint(&mut ctx, &mut rec);
        assert_eq!(w.chips[0].progress, 1.0, "snapped straight to visible");
        assert!(
            ctx.needs_layout(),
            "the snap changed a layout-relevant value, so it must request a relayout"
        );

        // The relayout a real shell would run on seeing `needs_layout`.
        layout(&mut w, AREA);
        assert!(
            w.chips[0].rect.width() > 0.0,
            "the chip's box reflects the snapped-in progress after the requested relayout"
        );
        assert_ne!(
            w.chips[0].delete,
            Rect::ZERO,
            "the delete glyph gets a real hit box, not an empty one"
        );
    }

    #[test]
    fn a_removed_chip_outlives_the_rebuild_that_dropped_it_and_then_settles_away() {
        let prev = view()
            .multi(true)
            .selected(vec!["apple".to_string(), "banana".to_string()]);
        let mut w = ready(&prev);
        settle(&mut w);
        assert_eq!(w.chip_count(), 2);

        let next = view().multi(true).selected(vec!["banana".to_string()]);
        rebuild(&mut w, &prev, &next);
        assert_eq!(
            w.chip_count(),
            2,
            "the removed chip is retained for its exit ramp"
        );
        assert!(w.chips[0].removing);
        settle(&mut w);
        assert_eq!(w.chip_count(), 1);
        assert_eq!(w.chips[0].value, "banana");
    }

    #[test]
    fn a_chip_re_added_mid_exit_is_revived_rather_than_duplicated() {
        let prev = view().multi(true).selected(vec!["apple".to_string()]);
        let mut w = ready(&prev);
        settle(&mut w);
        let gone = view().multi(true);
        rebuild(&mut w, &prev, &gone);
        assert!(w.chips[0].removing);
        rebuild(&mut w, &gone, &prev);
        assert_eq!(w.chip_count(), 1);
        assert!(!w.chips[0].removing);
    }

    #[test]
    fn a_chip_s_delete_glyph_requests_the_selection_without_that_value() {
        let mut w = ready(
            &view()
                .multi(true)
                .selected(vec!["apple".to_string(), "banana".to_string()]),
        );
        settle(&mut w);
        layout(&mut w, AREA);
        let mut state = AppState::default();
        let at = w.chips[0].delete.center();
        click(&mut w, &mut state, at);
        assert_eq!(state.selections, vec![vec!["banana".to_string()]]);
        assert!(state.opens.is_empty(), "a delete never toggles the panel");
    }

    #[test]
    fn a_press_on_the_chip_body_falls_through_to_the_field_s_own_toggle() {
        let mut w = ready(&view().multi(true).selected(vec!["apple".to_string()]));
        settle(&mut w);
        layout(&mut w, AREA);
        let mut state = AppState::default();
        let chip = w.chips[0].rect;
        let at = Point::new(chip.x0 + 2.0, chip.center().y);
        click(&mut w, &mut state, at);
        assert!(state.selections.is_empty());
        assert_eq!(state.opens, vec![true]);
    }

    #[test]
    fn a_disabled_field_s_chips_offer_no_delete_region() {
        let mut w = ready(
            &view()
                .multi(true)
                .enabled(false)
                .selected(vec!["apple".to_string()]),
        );
        settle(&mut w);
        layout(&mut w, AREA);
        assert_eq!(w.chips[0].delete, Rect::ZERO);
    }

    #[test]
    fn max_chips_hides_the_tail_behind_an_overflow_indicator() {
        let mut w = ready(&view().multi(true).max_chips(2).selected(vec![
            "apple".to_string(),
            "banana".to_string(),
            "cherry".to_string(),
        ]));
        assert_eq!(w.chip_count(), 2);
        assert_eq!(w.overflow.as_ref().map(|(_, n)| *n), Some(1));
        assert_eq!(
            w.overflow.as_ref().map(|(run, _)| run.content.as_str()),
            Some("+1")
        );
        settle(&mut w);
        layout(&mut w, AREA);
        let rec = paint(&mut w);
        // The container, two chips and the overflow pill.
        assert!(rec.rrects.len() >= 4);
    }

    // ---- paint ------------------------------------------------------------

    #[test]
    fn the_container_fills_with_surface_container_highest_at_the_panel_radius() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut w = ready(&view());
        let rec = paint(&mut w);
        let (origin, size, radius, color) = rec.rrects[0];
        assert_eq!(origin, Point::ORIGIN);
        assert_eq!(size, Size::new(w.width(), w.height()));
        assert_eq!(radius, theme.shape.extra_large);
        assert_eq!(color, theme.scheme().surface_container_highest);
    }

    #[test]
    fn a_press_washes_the_container_at_the_reference_s_pressed_alpha() {
        let mut w = ready(&view());
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, at.x, at.y));
        let pressed = paint(&mut w);
        assert_eq!(
            pressed.rrects[1].3.components[3],
            DROPDOWN_FIELD_PRESSED_ALPHA
        );
    }

    /// The hover link is recorded by the root's event pass and read back
    /// through `PaintCtx::is_hovered`, so a real [`RenderRoot`] is the only
    /// harness that can exercise it at all — the same shape
    /// [`crate::list_item`]'s own hover tests take.
    #[test]
    fn hovering_the_field_washes_it_at_the_reference_s_hover_alpha() {
        use frust_core::RenderRoot;
        let mut root: RenderRoot<AppState, DropdownFieldView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut AppState| view();
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(AREA, &mut tcx as &mut dyn Any);

        let mut rest = Recorder::default();
        root.paint(&mut rest, FrameTime::ZERO);
        assert_eq!(rest.washes(), Vec::<f32>::new(), "no wash at rest");

        root.event(&mut state, &pointer(PointerPhase::Move, 40.0, 20.0));
        let mut hovered = Recorder::default();
        root.paint(&mut hovered, FrameTime::ZERO);
        assert_eq!(hovered.washes(), vec![DROPDOWN_FIELD_HOVER_ALPHA]);
    }

    #[test]
    fn a_disabled_field_never_washes() {
        let mut w = ready(&view().enabled(false));
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, at.x, at.y));
        let rec = paint(&mut w);
        assert_eq!(rec.rrects.len(), 1, "the container and nothing else");
    }

    #[test]
    fn an_error_field_inks_its_content_in_the_error_role() {
        let theme = crate::baseline().with_brightness(Brightness::Light);
        let mut w = ready(&view().error(true).selected(vec!["apple".to_string()]));
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, at.x, at.y));
        let rec = paint(&mut w);
        let wash = rec.rrects[1].3;
        let error = theme.scheme().error;
        assert_eq!(wash.components[..3], error.components[..3]);
    }

    #[test]
    fn the_arrow_is_the_one_glyph_a_resting_field_paints() {
        let mut w = ready(&view());
        let rec = paint(&mut w);
        assert_eq!(rec.paths.len(), 1);
    }

    #[test]
    fn a_paint_with_no_hover_link_clears_the_latched_flag() {
        let mut w = ready(&view());
        let mut state = AppState::default();
        let at = Point::new(40.0, w.height() / 2.0);
        dispatch(&mut w, &mut state, &pointer(PointerPhase::Move, at.x, at.y));
        assert!(w.hovered);
        // `PaintCtx::for_test` reports no hover path.
        paint(&mut w);
        assert!(!w.hovered);
    }

    #[test]
    fn the_semantics_node_is_a_combo_box_carrying_the_open_state() {
        use frust_core::RenderRoot;
        let mut root: RenderRoot<AppState, DropdownFieldView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut AppState| view().open(true);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(AREA, &mut tcx as &mut dyn Any);
        let tree = root.semantics();
        let node = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::ComboBox)
            .expect("a combo-box node");
        assert!(node.1.is_expanded().unwrap_or(false));
    }
}
