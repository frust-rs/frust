//! `select`: the drawn select — a bordered trigger plus an anchored list of
//! options.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/select.tsx` (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) — a Radix
//! `Select` whose trigger is `flex w-fit items-center justify-between gap-2
//! rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-xs`
//! (`data-[size=default]:h-9 data-[size=sm]:h-8`, a `size-4 opacity-50`
//! `ChevronDownIcon`, `data-[placeholder]:text-muted-foreground`,
//! `dark:bg-input/30 dark:hover:bg-input/50`) and whose content is the popover
//! panel over a `p-1` viewport of `py-1.5 pr-8 pl-2 rounded-sm` items carrying a
//! `right-2` `CheckIcon` indicator, with `px-2 py-1.5 text-xs
//! text-muted-foreground` group labels.
//!
//! # Shape of the port
//!
//! The trigger's chrome is [`crate::input`]'s shared form-control border/ring
//! resolution plus [`crate::native_select`]'s shared chevron — the same two
//! pieces the native select is drawn from, so the two controls cannot drift —
//! and the list is [`crate::dropdown_menu`]'s shared menu list configured with a
//! **trailing** indicator gutter, muted labels, a height cap and the
//! open-on-the-current-value highlight. What is left here is what is actually the
//! select's own: the option data, the placeholder, and the mapping from a row
//! back to an option index.
//!
//! # Controlled, and mounted by the app
//!
//! `selected` is an index into `options` and the widget never moves it: a commit
//! is reported through `on_select` and the app feeds the new value back down.
//! Open/close is the app's too — it mounts [`select`] while its own flag is set,
//! or keeps it mounted and hands the flag to [`SelectView::open`] so a close
//! plays the panel's exit ramp (see [`crate::popover`] for both mount
//! contracts) — and [`select_trigger`] reports the toggle.
//!
//! # What the panel takes from its trigger
//!
//! `min-w-[var(--radix-select-trigger-width)]`: the panel reads the trigger's
//! captured rect (the anchor it is already placed against) and takes its width as
//! a floor, so a list never comes up narrower than the control it dropped out of.
//!
//! Two v1 gaps, both stated rather than worked around:
//!
//! * **No type-ahead.** Radix jumps the highlight to the option matching typed
//!   characters; the arrows and the pointer are the only ways to move it here.
//! * **The height cap is a constant.** Upstream's
//!   `max-h-(--radix-select-content-available-height)` is the space the host
//!   measured between the trigger and the viewport edge; the overlay host
//!   publishes no such measurement, so the list caps at
//!   [`SELECT_MAX_HEIGHT`] and scrolls (with the panel's own placement still
//!   flipping it above the trigger when that fits better).

use std::cell::RefCell;
use std::rc::Rc;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    Role, SemanticsCtx, Size, ThemeTextColor, View, Widget, any, build_child, erase_callback_arg,
    rebuild_child, route_event_single, teardown_child, visit_children,
};
use frust::{Theme, text};

use crate::components::dropdown_menu::{
    DropdownMenuItem, MenuIndicatorSide, MenuListStyle, dropdown_menu_item, dropdown_menu_label,
    menu_panel,
};
use crate::components::input::{
    FALLBACK, FieldChrome, input_border, paint_field_border, resolve_field_border,
};
use crate::components::native_select::{activates, draw_chevron};
use crate::components::popover::{PanelHandle, PanelStyle};
use crate::hit::{inside, presses};
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::style;

/// The list viewport's cap, in logical px — this port's stand-in for upstream's
/// available-height variable (see the [module docs](self)).
pub const SELECT_MAX_HEIGHT: f64 = 300.0;
/// `px-3` — the trigger's horizontal padding.
const TRIGGER_PAD_X: f64 = 12.0;
/// `gap-2` — the gap between the trigger's label and its chevron.
const TRIGGER_GAP: f64 = 8.0;
/// `opacity-50` on the trigger's chevron.
const CHEVRON_OPACITY: f32 = 0.5;
/// Alpha of the dark-mode hover wash: `dark:hover:bg-input/50`.
const DARK_HOVER_ALPHA: f32 = 0.5;
/// Alpha of the dark-mode resting fill: `dark:bg-input/30`.
const DARK_FILL_ALPHA: f32 = 0.3;

/// The trigger's height variants: `default` (`h-9`) and `sm` (`h-8`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectSize {
    /// `data-[size=default]:h-9`.
    #[default]
    Default,
    /// `data-[size=sm]:h-8`.
    Sm,
}

impl SelectSize {
    /// This size's trigger height, in logical px.
    pub fn height(self) -> f64 {
        match self {
            SelectSize::Default => style::HEIGHT_DEFAULT,
            SelectSize::Sm => style::HEIGHT_SM,
        }
    }
}

/// One option in a [`select`] list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    label: String,
    group: Option<String>,
    disabled: bool,
}

/// Create an option labelled `label`.
pub fn select_option(label: impl Into<String>) -> SelectOption {
    SelectOption {
        label: label.into(),
        group: None,
        disabled: false,
    }
}

impl SelectOption {
    /// Put the option under a named group — consecutive options sharing a group
    /// name render under one `SelectLabel` heading.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Make the option unselectable (`data-[disabled]`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The option's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// A declarative shadcn select list. See [`select`].
pub struct SelectView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    style: PanelHandle,
    placement: OverlayPlacement,
}

/// The list style every select panel uses: the check gutter on the trailing side,
/// the select's muted labels, a capped viewport, and the highlight opening on the
/// current value.
fn list_style() -> MenuListStyle {
    MenuListStyle {
        indicator_side: MenuIndicatorSide::Trailing,
        muted_labels: true,
        max_height: Some(SELECT_MAX_HEIGHT),
        highlight_indicated: true,
    }
}

/// The menu rows for `options`, plus the option index each row maps back to.
fn rows_for(
    options: &[SelectOption],
    selected: Option<usize>,
) -> (Vec<DropdownMenuItem>, Vec<usize>) {
    let mut items = Vec::new();
    let mut map = Vec::new();
    let mut group: Option<&str> = None;
    for (i, option) in options.iter().enumerate() {
        let option_group = option.group.as_deref();
        if option_group != group {
            if let Some(name) = option_group {
                items.push(dropdown_menu_label(name));
                // A label takes an index of its own in the depth-first numbering
                // (`crate::dropdown_menu`), so the map has to carry a slot for it.
                map.push(usize::MAX);
            }
            group = option_group;
        }
        items.push(
            dropdown_menu_item(option.label.clone())
                .disabled(option.disabled)
                .checked(selected == Some(i)),
        );
        map.push(i);
    }
    (items, map)
}

/// Build a select list over `options`, showing `options[selected]` as checked and
/// reporting a commit through `on_select(state, option_index)`.
///
/// Mount it while the app's own open flag is set, or keep it mounted and hand
/// the flag to [`SelectView::open`] for an exit ramp — anchored to the trigger's
/// captured rect either way (see the [module docs](self)).
pub fn select<State: 'static, F: Fn(&mut State, usize) + 'static>(
    options: Vec<SelectOption>,
    selected: Option<usize>,
    on_select: F,
) -> SelectView<State> {
    let (items, map) = rows_for(&options, selected);
    let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::menu()));
    // The rows are numbered over every row, labels included; the map turns one
    // back into the option index the caller knows.
    let commit = move |state: &mut State, row: usize| {
        if let Some(option) = map.get(row).copied()
            && option != usize::MAX
        {
            on_select(state, option);
        }
    };
    let content = menu_panel(items, list_style(), style.clone(), Rc::new(commit));
    // The list lines up with the trigger's leading edge, not its centre.
    let placement = OverlayPlacement::default().align(OverlayAlign::Start);
    SelectView {
        inner: anchored(content).placement(placement),
        style,
        placement,
    }
}

impl<State: 'static> SelectView<State> {
    /// Anchor the list to the trigger's captured rect — which is also where its
    /// minimum width comes from.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.style.borrow_mut().anchor = Some(anchor.clone());
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the list opens on (default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.apply_placement()
    }

    /// Set the cross-axis alignment (default `start`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.apply_placement()
    }

    /// Set the gap between trigger and list (`sideOffset`, default `4`).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self.apply_placement()
    }

    /// Hand a **kept-mounted** list the app's open flag, so closing it plays the
    /// panel's exit ramp instead of vanishing (see [`crate::popover`]).
    ///
    /// The default is `true`: a mounted list is an open one. A commit still
    /// reports through `on_select` on the press that made it — only the pixels
    /// linger.
    pub fn open(mut self, open: bool) -> Self {
        self.style.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: a press outside the list or a focus-routed
    /// Escape reports `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.inner = self
            .inner
            .on_dismiss(move |state| on_open_change(state, false));
        self
    }

    /// Re-hand the current placement to the host.
    fn apply_placement(mut self) -> Self {
        self.inner = self.inner.placement(self.placement);
        self
    }
}

impl<State: 'static> View<State> for SelectView<State> {
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

/// A view-held, typed open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// A declarative shadcn select trigger. See [`select_trigger`].
pub struct SelectTriggerView<State: 'static> {
    options: Vec<SelectOption>,
    selected: Option<usize>,
    placeholder: String,
    size: SelectSize,
    invalid: bool,
    disabled: bool,
    open: bool,
    anchor: OverlayAnchor,
    on_open_change: OnOpenChange<State>,
}

/// Create the select's trigger: the closed control showing
/// `options[selected]`, capturing its own rect into `anchor`, and reporting a
/// click as an open toggle.
pub fn select_trigger<State: 'static>(
    anchor: &OverlayAnchor,
    options: Vec<SelectOption>,
    selected: Option<usize>,
) -> SelectTriggerView<State> {
    SelectTriggerView {
        options,
        selected,
        placeholder: String::new(),
        size: SelectSize::default(),
        invalid: false,
        disabled: false,
        open: false,
        anchor: anchor.clone(),
        on_open_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> SelectTriggerView<State> {
    /// Set the text shown while nothing is selected
    /// (`data-[placeholder]:text-muted-foreground`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set the control height (`default`/`sm`).
    pub fn size(mut self, size: SelectSize) -> Self {
        self.size = size;
        self
    }

    /// Put the control in its `aria-invalid` state.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Disable the control: inert, dimmed, [`style::DISABLED_CURSOR`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Tell the trigger whether its list is open, so a click reports the other
    /// state.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Set the open-change callback: `!open` on a release inside the trigger, or
    /// on Space/Enter while focused.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }

    /// The label currently displayed, and whether it is the placeholder.
    fn label(&self) -> (String, bool) {
        match self.selected.and_then(|i| self.options.get(i)) {
            Some(option) => (option.label.clone(), false),
            None => (self.placeholder.clone(), true),
        }
    }

    /// The label's text child.
    fn label_view(&self) -> AnyView<State> {
        let (label, is_placeholder) = self.label();
        let role = if is_placeholder {
            ThemeTextColor::OnSurfaceVariant
        } else {
            ThemeTextColor::OnSurface
        };
        any(text(label)
            .size(style::TEXT_SM as f32)
            .family(crate::tokens::sans_family())
            .themed_role(role))
    }
}

/// The retained widget for a [`SelectTriggerView`].
pub struct SelectTriggerWidget {
    label: ChildPod,
    /// The option labels, for the semantics node's value.
    options: Vec<String>,
    selected: Option<usize>,
    size: SelectSize,
    invalid: bool,
    disabled: bool,
    open: bool,
    anchor: OverlayAnchor,
    /// The latched hover flag (self-corrected from `PaintCtx::is_hovered`).
    hovered: bool,
    captured: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl<State: 'static> View<State> for SelectTriggerView<State> {
    type Element = SelectTriggerWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SelectTriggerWidget {
        SelectTriggerWidget {
            label: build_child(&self.label_view(), ctx),
            options: self.options.iter().map(|o| o.label.clone()).collect(),
            selected: self.selected,
            size: self.size,
            invalid: self.invalid,
            disabled: self.disabled,
            open: self.open,
            anchor: self.anchor.clone(),
            hovered: false,
            captured: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectTriggerWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.label_view(),
            &self.label_view(),
            &mut element.label,
            ctx,
        );
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.invalid != self.invalid
            || element.disabled != self.disabled
            || element.selected != self.selected
            || element.open != self.open
        {
            element.invalid = self.invalid;
            element.disabled = self.disabled;
            element.selected = self.selected;
            element.open = self.open;
            flags |= ChangeFlags::PAINT;
        }
        element.options = self.options.iter().map(|o| o.label.clone()).collect();
        element.anchor = self.anchor.clone();
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, element: &mut SelectTriggerWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.label_view(), &mut element.label, ctx);
    }
}

impl SelectTriggerWidget {
    /// Report the open state an activation asks for. The widget never flips its
    /// own flag — the next rebuild does, from the app.
    fn toggle(&mut self, ctx: &mut EventCtx) {
        let next = !self.open;
        (self.on_open_change)(ctx, next);
    }
}

impl Widget for SelectTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let height = self.size.height();
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // `px-3 gap-2` with the chevron pinned to the trailing edge.
        let reserved = 2.0 * TRIGGER_PAD_X + TRIGGER_GAP + style::ICON_SIZE;
        let label_bc =
            BoxConstraints::new(Size::ZERO, Size::new((width - reserved).max(0.0), height));
        let label = self.label.layout_child(ctx, &label_bc);
        self.label.set_origin(Point::new(
            TRIGGER_PAD_X,
            ((height - label.height) / 2.0).max(0.0),
        ));
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::is_hovered` is authoritative — it drops the wash on the frame
        // the pointer moves onto something else, which no event can report.
        self.hovered = ctx.is_hovered();
        // `PaintCtx::origin` is absolute window space: the rect the list is placed
        // against, and the floor its width takes.
        self.anchor
            .set(Rect::from_origin_size(ctx.origin(), ctx.size()));

        let chrome = FieldChrome {
            focused: ctx.has_focus() && !self.disabled,
            invalid: self.invalid,
            disabled: self.disabled,
        };
        let (origin, size) = (ctx.origin(), ctx.size());
        let theme = Theme::from_paint_ctx(ctx);
        let resolved = resolve_field_border(theme, chrome);
        let chevron = style::disabled_tint(
            style::with_alpha(muted_foreground(theme), CHEVRON_OPACITY),
            self.disabled,
        );
        // `dark:bg-input/30 dark:hover:bg-input/50` — a fill the light class list
        // does not have at all (`bg-transparent` there).
        let fill = style::is_dark(theme).then(|| {
            let alpha = if self.hovered && !self.disabled {
                DARK_HOVER_ALPHA
            } else {
                DARK_FILL_ALPHA
            };
            style::with_alpha(input_border(theme), alpha)
        });

        style::draw_shadow(
            scene,
            origin,
            size,
            resolved.radius,
            style::SHADOW_XS,
            theme,
        );
        if let Some(fill) = fill {
            scene.fill_rounded_rect(origin, size, resolved.radius, fill);
        }
        self.label.paint_child(ctx, scene);
        draw_chevron(
            scene,
            Point::new(
                origin.x + size.width - TRIGGER_PAD_X - style::ICON_SIZE / 2.0,
                origin.y + size.height / 2.0,
            ),
            style::ICON_SIZE,
            0.0,
            chevron,
        );
        paint_field_border(scene, origin, size, resolved);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The label is a text leaf and consumes nothing, but a container still
        // forwards (a broadcast must reach it, and it keeps the pod live).
        let routed = route_event_single(&mut self.label, ctx, event);
        if event.is_broadcast() {
            return routed;
        }
        if let InputEvent::Key(key) = event {
            if self.disabled || !activates(&key.key) {
                return EventResult::Ignored;
            }
            self.toggle(ctx);
            return EventResult::Handled;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        if self.disabled {
            // Upstream keeps pointer events on a disabled trigger
            // (`disabled:cursor-not-allowed`, no `pointer-events-none`).
            if p.phase == PointerPhase::Move && inside(p.position, ctx.size()) {
                ctx.set_cursor(style::DISABLED_CURSOR);
            }
            return EventResult::Ignored;
        }
        let over = inside(p.position, ctx.size());
        match p.phase {
            PointerPhase::Down if over && presses(p) => {
                self.captured = true;
                ctx.capture_pointer();
                // Focus is what makes the ring appear, routes Space/Enter here,
                // and lets Escape reach the list's host afterwards.
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                // Claimed after the routing above (the claim-ordering rule).
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                if self.captured {
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            PointerPhase::Up if self.captured => {
                self.captured = false;
                if over {
                    self.toggle(ctx);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            // A `Cancel` arm clears its own flags and touches no app state.
            PointerPhase::Cancel if self.captured => {
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let value = self
            .selected
            .and_then(|i| self.options.get(i))
            .cloned()
            .unwrap_or_default();
        ctx.push_container(
            Role::ComboBox,
            |node| {
                node.set_value(value.clone());
                if self.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            },
            |ctx| self.label.semantics_child(ctx),
        );
    }

    visit_children!(label);
}

/// The `muted-foreground` token: themed `on_surface_variant`, else the fallback
/// table.
fn muted_foreground(theme: Option<&Theme>) -> Color {
    theme.map_or(FALLBACK.muted_foreground, |t| t.scheme().on_surface_variant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{
        Recorder, WINDOW, escape, ft_ms, key_event, light, pointer,
    };
    use frust::authoring::text::TextContext;
    use frust::authoring::{Key, NamedKey};
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        open: bool,
        opens: Vec<bool>,
        selected: Option<usize>,
        commits: Vec<usize>,
    }

    fn options() -> Vec<SelectOption> {
        vec![
            select_option("Apple").group("Fruits"),
            select_option("Banana").group("Fruits"),
            select_option("Blueberry").group("Fruits").disabled(true),
            select_option("Carrot").group("Vegetables"),
        ]
    }

    /// `duration-200`, the shared ramp the panel exits over.
    const RAMP_MS: f64 = 200.0;

    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
        /// Whether the list is kept mounted and handed the flag (the mount an
        /// exit ramp needs) rather than mounted only while open.
        kept: bool,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::with_mount(false)
        }

        fn kept_mounted() -> Self {
            Self::with_mount(true)
        }

        fn with_mount(kept: bool) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: AppState::default(),
                tcx: TextContext::new(),
                anchor: OverlayAnchor::new(),
                kept,
                clock: 0.0,
            };
            h.root.set_theme(Box::new(light()));
            h.pass();
            h
        }

        fn pass(&mut self) {
            let anchor = self.anchor.clone();
            let kept = self.kept;
            let mut logic = move |state: &mut AppState| {
                let trigger = select_trigger(&anchor, options(), state.selected)
                    .placeholder("Select a fruit")
                    .open(state.open)
                    .on_open_change(|s: &mut AppState, open| {
                        s.opens.push(open);
                        s.open = open;
                    });
                let mut children = vec![any(trigger)];
                if kept || state.open {
                    children.push(any(select(
                        options(),
                        state.selected,
                        |s: &mut AppState, option| {
                            s.commits.push(option);
                            s.selected = Some(option);
                            s.open = false;
                        },
                    )
                    .anchor(&anchor)
                    .open(state.open)
                    .on_open_change(|s: &mut AppState, open| {
                        s.opens.push(open);
                        s.open = open;
                    })));
                }
                frust::Stack(children)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        /// Paint at `ms` without rebuilding — the ramp's own frames.
        fn paint_at(&mut self, ms: f64) -> Recorder {
            self.clock = ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        /// Whether the list's `bg-popover` panel was drawn.
        fn panel_painted(rec: &Recorder) -> bool {
            let theme = light();
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == theme.scheme().surface_container_high)
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
            self.pass();
        }

        fn click(&mut self, x: f64, y: f64) {
            self.event(pointer(PointerPhase::Down, x, y));
            self.event(pointer(PointerPhase::Up, x, y));
        }
    }

    /// The centre of the list's first item row, which sits under the panel's own
    /// first row (a group label).
    fn first_row(h: &Harness) -> Point {
        let trigger = h.anchor.rect();
        Point::new(
            trigger.x0 + 20.0,
            trigger.y1
                + crate::overlay::SIDE_OFFSET
                + crate::components::popover::MENU_PADDING
                + 24.0
                + 14.0,
        )
    }

    #[test]
    fn the_rows_carry_a_label_per_group_and_map_back_to_option_indices() {
        let (items, map) = rows_for(&options(), Some(1));
        assert_eq!(items.len(), 6, "four options under two group labels");
        assert_eq!(map, vec![usize::MAX, 0, 1, 2, usize::MAX, 3]);
        assert_eq!(items[2].label(), "Banana");
    }

    #[test]
    fn the_trigger_is_h9_shows_the_placeholder_and_toggles_on_click() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        assert_eq!(h.state.opens, vec![true]);
        assert!(h.state.open);
        // A second click on the trigger arrives as a dismiss of the open list.
        h.click(40.0, 18.0);
        assert_eq!(h.state.opens, vec![true, false]);
    }

    #[test]
    fn space_activates_a_focused_trigger_without_a_click() {
        let mut h = Harness::new();
        // A press focuses the trigger; the cancel drops it without toggling.
        h.event(pointer(PointerPhase::Down, 40.0, 18.0));
        h.event(pointer(PointerPhase::Cancel, 40.0, 18.0));
        assert_eq!(h.state.opens, Vec::<bool>::new());
        h.event(key_event(Key::Character(" ".to_string())));
        assert_eq!(h.state.opens, vec![true], "Space opens the list");
    }

    #[test]
    fn escape_closes_an_open_list_once_a_press_has_focused_it() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        assert!(h.state.open);
        // Escape needs a focus chain into the overlay, and the framework has no
        // focus-on-appear hook: the trigger still holds focus here.
        h.event(escape());
        assert!(h.state.open, "no chain into the list yet");

        // One press inside the list (no release, so nothing commits) records it.
        let row = first_row(&h);
        h.event(pointer(PointerPhase::Down, row.x, row.y));
        h.event(escape());
        assert!(!h.state.opens.last().unwrap(), "the last report is a close");
    }

    #[test]
    fn arrows_move_the_highlight_and_enter_commits_an_option_index() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        // A press with no release focuses the list and highlights its row.
        let row = first_row(&h);
        h.event(pointer(PointerPhase::Down, row.x, row.y));
        assert_eq!(h.state.commits, Vec::<usize>::new(), "never on down");
        h.event(key_event(Key::Named(NamedKey::ArrowDown)));
        h.event(key_event(Key::Named(NamedKey::Enter)));
        assert_eq!(h.state.commits, vec![1], "`Banana`, the second option");
        assert_eq!(h.state.selected, Some(1));
    }

    #[test]
    fn the_highlight_skips_the_disabled_option_and_the_group_labels() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        let row = first_row(&h);
        h.event(pointer(PointerPhase::Down, row.x, row.y));
        for _ in 0..2 {
            h.event(key_event(Key::Named(NamedKey::ArrowDown)));
        }
        h.event(key_event(Key::Named(NamedKey::Enter)));
        assert_eq!(
            h.state.commits,
            vec![3],
            "`Carrot`: the disabled option and the second group label are skipped"
        );
    }

    #[test]
    fn a_commit_reports_at_once_and_the_kept_mounted_list_paints_itself_out() {
        let mut h = Harness::kept_mounted();
        h.click(40.0, 18.0);
        h.paint_at(RAMP_MS * 2.0);
        assert!(Harness::panel_painted(&h.paint_at(h.clock)));

        // Selecting commits and closes on the release itself.
        let row = first_row(&h);
        h.click(row.x, row.y);
        assert_eq!(h.state.commits, vec![0], "`Apple`, reported at once");
        assert!(!h.state.open, "and the app is already closed");

        let start = h.clock;
        let mid = h.paint_at(start + RAMP_MS / 2.0);
        assert!(Harness::panel_painted(&mid), "the list is still on screen");
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);

        // Nothing lands on the closing list.
        let before = h.state.commits.len();
        let outcome = h
            .root
            .event(&mut h.state, &pointer(PointerPhase::Down, row.x, row.y));
        assert!(outcome.handled, "a closing list swallows a press that lands on it");
        assert_eq!(h.state.commits.len(), before);

        assert!(
            !Harness::panel_painted(&h.paint_at(start + RAMP_MS * 2.0)),
            "gone at settle"
        );
    }

    #[test]
    fn the_panel_is_at_least_as_wide_as_the_trigger() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(0.0, 0.0, 260.0, style::HEIGHT_DEFAULT));
        let view: SelectView<AppState> =
            select(options(), Some(0), |_s: &mut AppState, _i| {}).anchor(&anchor);
        let mut counter = 0u64;
        let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
        assert_eq!(
            w.content_rect().width(),
            260.0,
            "min-w-[var(--radix-select-trigger-width)]"
        );
    }

    #[test]
    fn the_trigger_paints_the_field_border_and_a_muted_chevron() {
        let mut h = Harness::new();
        let mut rec = Recorder::default();
        h.root.paint(&mut rec, ft_ms(0.0));
        let theme = light();
        assert!(
            rec.strokes
                .iter()
                .any(|(_, w, c)| *w == style::BORDER_WIDTH && *c == theme.scheme().outline_variant),
            "border-input"
        );
        assert!(
            rec.strokes
                .iter()
                .any(|(_, _, c)| c.components[3] == CHEVRON_OPACITY),
            "the size-4 opacity-50 chevron"
        );
        assert!(
            rec.shadows
                .iter()
                .any(|(_, _, _, sd, _)| *sd == style::SHADOW_XS.std_dev),
            "shadow-xs"
        );
    }

    #[test]
    fn a_disabled_trigger_is_inert_and_asks_not_allowed() {
        let anchor = OverlayAnchor::new();
        let mut root: RenderRoot<AppState, SelectTriggerView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let a = anchor.clone();
        let mut logic = move |_s: &mut AppState| {
            select_trigger(&a, options(), None)
                .disabled(true)
                .on_open_change(|s: &mut AppState, open| s.opens.push(open))
        };
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        root.event(&mut state, &pointer(PointerPhase::Down, 40.0, 18.0));
        root.event(&mut state, &pointer(PointerPhase::Up, 40.0, 18.0));
        assert_eq!(state.opens, Vec::<bool>::new());
        root.event(&mut state, &pointer(PointerPhase::Move, 40.0, 18.0));
        assert_eq!(root.cursor(), style::DISABLED_CURSOR);
    }

    #[test]
    fn semantics_is_a_combobox_carrying_the_selected_option() {
        let anchor = OverlayAnchor::new();
        let mut root: RenderRoot<AppState, SelectTriggerView<AppState>> = RenderRoot::new();
        let mut state = AppState::default();
        let a = anchor.clone();
        let mut logic = move |_s: &mut AppState| select_trigger(&a, options(), Some(1));
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::ComboBox)
            .expect("a ComboBox node");
        assert_eq!(node.value(), Some("Banana"));
        assert!(node.supports_action(Action::Click));
    }
}
