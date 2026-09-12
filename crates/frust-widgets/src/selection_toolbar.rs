//! The baseline framework-drawn selection toolbar: a horizontal pill of text
//! buttons — Cut, Copy, Paste, Select all — showing only the verbs
//! [`SelectionToolbarRequest::actions`](frust_core::SelectionToolbarRequest::actions)
//! enables.
//!
//! [`selection_toolbar`] is the view-fn a text field's own
//! [`OverlaySlot`](crate::overlay::OverlaySlot) floats under
//! [`SelectionToolbarPolicy::Framework`](frust_core::SelectionToolbarPolicy::Framework)
//! (see `frust_core::selection_toolbar`'s module docs for the two-route
//! seam): it returns an [`AnyView<()>`] — the pod carries no application
//! state, so the view built here never reads anything but the request it was
//! handed — and is installed as the process's default builder at bootstrap
//! (`frust::__install_default_selection_toolbar`), set-if-unset so a design
//! system that installed its own first keeps it.
//!
//! # Shape
//!
//! One [`ToolbarButtonWidget`] per enabled verb, laid out left to right with
//! no gap, wrapped in a single pill: `surface_container` fill (themed;
//! [`FALLBACK_FILL`] unthemed), a 1px `outline_variant` border
//! ([`FALLBACK_BORDER`] unthemed), [`RADIUS`] corners. Each item pads its
//! `on_surface`-colored label (`text`'s own default role — see
//! [`crate::text`]) by [`PAD_X`]/[`PAD_Y`] and is at least [`MIN_HEIGHT`]
//! logical px tall; the pill itself is never padded or bordered beyond that —
//! it hugs its items exactly, since the [`OverlaySlot`](crate::overlay::OverlaySlot)
//! that hosts it, not this view, decides where it sits (module docs: "sized
//! to content").
//!
//! A tap follows every other interactive baseline widget's fire-on-up-inside
//! contract ([`crate::button`]/[`crate::icon_button`]'s own module docs): a
//! primary-button `Down` inside captures the pointer and shows a pressed wash
//! at [`crate::authoring::PRESSED_OPACITY`]; the verb only fires on an `Up`
//! that lands back inside. Cut/Copy/Select all ride
//! [`EventCtx::dispatch_edit_command`](frust_core::EventCtx::dispatch_edit_command)
//! (the toolbar and the field it acts on are two different widgets connected
//! only through the overlay portal, not a container path — see that method's
//! own "why this is not just an `InputEvent::EditCommand`" section); Paste
//! rides [`EventCtx::request_paste`](frust_core::EventCtx::request_paste)
//! instead, since only the shell may read the host clipboard.
//!
//! # Labels
//!
//! English-only ([`LABELS`]): localisation is deferred to the builder seam —
//! a design system (or an app) installing its own translated builder via
//! [`frust_core::set_selection_toolbar_builder`]/
//! [`frust_core::install_selection_toolbar_builder_if_unset`] is how this
//! baseline is meant to be replaced for a localized app. This baseline never
//! localises its own four labels.

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EditCommand, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SelectionToolbarActions,
    SelectionToolbarRequest, SemanticsCtx, View, Widget, any,
};
use frust_theme::Theme;
use kurbo::{Point, RoundedRect, Shape, Size};
use peniko::{Brush, Color};

use crate::ChildKey;
use crate::authoring::{PRESSED_OPACITY, presses};

/// Minimum tap-target height for one toolbar item, in logical px — enforced
/// per item (not just on the pill as a whole) so the pill's own height, which
/// is driven by its tallest child, always clears it too.
const MIN_HEIGHT: f64 = 44.0;
/// Horizontal padding around each item's label, in logical px (mirrors
/// [`crate::button`]'s identically-valued `PAD_X` — no shared spacing-token
/// constant exists yet to resolve this from instead; see that module's own
/// `PAD_X` doc comment).
const PAD_X: f64 = 12.0;
/// Vertical padding around each item's label, in logical px (mirrors
/// [`crate::button`]'s identically-valued `PAD_Y`; see [`PAD_X`]).
const PAD_Y: f64 = 8.0;
/// Pill corner radius, in logical px.
const RADIUS: f64 = 8.0;
/// Pill border stroke width, in logical px (a hairline, matching
/// [`crate::button`]/[`crate::container`]'s identical precedent).
const BORDER_WIDTH: f64 = 1.0;
/// Flattening tolerance for the border's rounded-rect stroke path — see
/// [`crate::button`]'s identical precedent/rationale.
const BORDER_TOLERANCE: f64 = 0.1;

/// Unthemed-fallback pill fill (a light neutral surface; no M3 anchor exists
/// without a theme to resolve `surface_container` from).
const FALLBACK_FILL: Color = Color::from_rgb8(0xF0, 0xF0, 0xF0);
/// Unthemed-fallback pill border (see [`FALLBACK_FILL`]'s no-anchor note).
const FALLBACK_BORDER: Color = Color::from_rgb8(0xD0, 0xD0, 0xD0);
/// Unthemed-fallback pressed-wash ink — matches [`crate::text::TextView`]'s
/// own unthemed default color (`TextStyle::default`'s black), since an
/// unthemed label paints in that color and the wash is meant to darken it.
const FALLBACK_INK: Color = Color::from_rgb8(0x00, 0x00, 0x00);

/// English label strings for each selection-toolbar verb, in the pill's fixed
/// display order (Cut, Copy, Paste, Select all). See the [module docs](self)'
/// "Labels" section: localisation rides the builder seam, not this table.
struct ToolbarLabels {
    cut: &'static str,
    copy: &'static str,
    paste: &'static str,
    select_all: &'static str,
}

/// The one English label table this baseline ever reads from — see
/// [`ToolbarLabels`].
const LABELS: ToolbarLabels = ToolbarLabels {
    cut: "Cut",
    copy: "Copy",
    paste: "Paste",
    select_all: "Select all",
};

/// One verb a toolbar item fires — the per-item half of
/// [`frust_core::SelectionToolbarActions`]' enabled set, plus the label it
/// renders and the [`EventCtx`] call it makes on an up-inside release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ToolbarAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The pill's resolved chrome colors — themed: `surface_container`/
/// `outline_variant`; unthemed: [`FALLBACK_FILL`]/[`FALLBACK_BORDER`] exactly.
/// Mirrors `TextInput`'s `Chrome::resolve` shape
/// (`crates/frust-widgets/src/textinput.rs`), theme-first with a fallback
/// constant.
struct ToolbarChrome {
    fill: Color,
    border: Color,
}

impl ToolbarChrome {
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let s = theme.scheme();
                ToolbarChrome {
                    fill: s.surface_container,
                    border: s.outline_variant,
                }
            }
            None => ToolbarChrome {
                fill: FALLBACK_FILL,
                border: FALLBACK_BORDER,
            },
        }
    }
}

/// One reconcilable pill item: its verb (also its [`ChildKey`], so toggling
/// which verbs are enabled — a selection shrinking to nothing selectable,
/// say — relocates a surviving item's live widget instead of rebuilding
/// whatever now sits at its index) and its erased view.
struct ToolbarChild {
    action: ToolbarAction,
    view: AnyView<()>,
}

/// The enabled items, in the pill's fixed display order (Cut, Copy, Paste,
/// Select all — this deliverable's own title and the [module docs](self)).
fn toolbar_children(actions: SelectionToolbarActions) -> Vec<ToolbarChild> {
    let mut children = Vec::with_capacity(4);
    if actions.cut {
        children.push(ToolbarChild {
            action: ToolbarAction::Cut,
            view: item_view(ToolbarAction::Cut, LABELS.cut),
        });
    }
    if actions.copy {
        children.push(ToolbarChild {
            action: ToolbarAction::Copy,
            view: item_view(ToolbarAction::Copy, LABELS.copy),
        });
    }
    if actions.paste {
        children.push(ToolbarChild {
            action: ToolbarAction::Paste,
            view: item_view(ToolbarAction::Paste, LABELS.paste),
        });
    }
    if actions.select_all {
        children.push(ToolbarChild {
            action: ToolbarAction::SelectAll,
            view: item_view(ToolbarAction::SelectAll, LABELS.select_all),
        });
    }
    children
}

/// Erase one [`ToolbarButtonView`] into an `AnyView<()>`.
fn item_view(action: ToolbarAction, label: &'static str) -> AnyView<()> {
    any(ToolbarButtonView { action, label })
}

/// Build this item's label child view — a plain [`crate::text::text`] run,
/// left at its default [`crate::text::ThemeTextColor::OnSurface`] role (this
/// deliverable's own "on_surface labels").
fn label_view(label: &'static str) -> AnyView<()> {
    any::<(), _>(crate::text::text(label))
}

/// The declarative one-item pressable text button. See the [module docs](self).
struct ToolbarButtonView {
    action: ToolbarAction,
    label: &'static str,
}

impl View<()> for ToolbarButtonView {
    type Element = ToolbarButtonWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ToolbarButtonWidget {
        let view = label_view(self.label);
        ToolbarButtonWidget {
            action: self.action,
            label: self.label,
            text: crate::authoring::build_child(&view, ctx),
            pressed: false,
            captured: false,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ToolbarButtonWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.action = self.action;
        element.label = self.label;
        let prev_view = label_view(prev.label);
        let next_view = label_view(self.label);
        crate::authoring::rebuild_child(&prev_view, &next_view, &mut element.text, ctx)
    }

    fn teardown(&self, element: &mut ToolbarButtonWidget, ctx: &mut BuildCtx<'_>) {
        let view = label_view(self.label);
        crate::authoring::teardown_child(&view, &mut element.text, ctx);
    }
}

/// The retained per-item widget: a text label child plus the press state
/// machine every baseline interactive widget in this crate shares (see the
/// [module docs](self)).
struct ToolbarButtonWidget {
    action: ToolbarAction,
    /// The accessible label (also what [`Widget::semantics`] names the node
    /// with); kept alongside the rendered [`Self::text`] pod the same way
    /// [`crate::icon_button::IconButtonWidget`] keeps its own `label` field.
    label: &'static str,
    text: ChildPod,
    /// The pressed *visual* state (the wash paints); purely cosmetic.
    pressed: bool,
    /// Armed by a `Down` alongside `capture_pointer`, cleared on `Up`/
    /// `Cancel` — gates `Move`/`Up` so an uncaptured hover `Move` never
    /// latches `pressed` or fires (mirrors
    /// [`crate::icon_button::IconButtonWidget`]'s identical field).
    captured: bool,
}

/// Whether widget-local `pos` lies within a `size`-sized box anchored at the
/// origin (mirrors [`crate::button`]/[`crate::icon_button`]'s
/// identically-named helper).
fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`crate::button`]/[`crate::icon_button`]'s identically-named helper).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

impl Widget for ToolbarButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let inset = Size::new(PAD_X * 2.0, PAD_Y * 2.0);
        let inner_max = Size::new(
            (bc.max().width - inset.width).max(0.0),
            (bc.max().height - inset.height).max(0.0),
        );
        let label_size = self
            .text
            .layout_child(ctx, &BoxConstraints::loose(inner_max));
        let width = label_size.width + inset.width;
        let height = (label_size.height + inset.height).max(MIN_HEIGHT);
        self.text
            .set_origin(Point::new(PAD_X, (height - label_size.height) / 2.0));
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let ink = match theme {
            Some(theme) => theme.scheme().on_surface,
            None => FALLBACK_INK,
        };
        if self.pressed {
            scene.fill_rect(ctx.origin(), ctx.size(), with_alpha(ink, PRESSED_OPACITY));
        }
        self.text.paint_child(ctx, scene);
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
                self.pressed = true;
                self.captured = true;
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = inside(p.position, ctx.size());
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                // Fire on up-inside only.
                if inside(p.position, ctx.size()) {
                    match self.action {
                        ToolbarAction::Cut => ctx.dispatch_edit_command(EditCommand::Cut),
                        ToolbarAction::Copy => ctx.dispatch_edit_command(EditCommand::Copy),
                        ToolbarAction::SelectAll => {
                            ctx.dispatch_edit_command(EditCommand::SelectAll)
                        }
                        // Only the shell may read the host clipboard, so Paste
                        // asks for it instead of dispatching a command a field
                        // could apply directly — see `EventCtx::request_paste`.
                        ToolbarAction::Paste => ctx.request_paste(),
                    }
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // One a11y node per item (Role::Button), named by its own label.
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label);
            node.add_action(Action::Click);
        });
    }

    crate::authoring::visit_children!(text);
}

/// Build the baseline framework-drawn selection-toolbar view for `request`: a
/// pill of text buttons for whichever verbs
/// [`request.actions`](SelectionToolbarRequest::actions) enables. See the
/// [module docs](self).
///
/// The pod is over `()` (see `frust_core::selection_toolbar`'s module docs
/// for why an overlay pod carries no application state): the returned view
/// never reads anything but `request` itself, so it is usable from a field
/// hosted under any app state at all. Sized to its content — the
/// [`OverlaySlot`](crate::overlay::OverlaySlot) that floats it, via
/// [`crate::overlay::place`], decides where that content sits.
///
/// [`request.present_menu`](SelectionToolbarRequest::present_menu) is ignored
/// here, and that is not an oversight: a builder is called only for a bar the
/// field already wants, so on this route the flag says nothing the call itself
/// has not. It exists for the platform route, where "which verbs apply" is a
/// level a host reads at any moment and "put a menu up" is an edge.
pub fn selection_toolbar(request: &SelectionToolbarRequest) -> AnyView<()> {
    any(SelectionToolbarView {
        actions: request.actions,
    })
}

struct SelectionToolbarView {
    actions: SelectionToolbarActions,
}

impl View<()> for SelectionToolbarView {
    type Element = SelectionToolbarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SelectionToolbarWidget {
        let children = toolbar_children(self.actions);
        let mut items = Vec::with_capacity(children.len());
        for child in &children {
            items.push(crate::authoring::build_child(&child.view, ctx));
        }
        SelectionToolbarWidget { items }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SelectionToolbarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let prev_children = toolbar_children(prev.actions);
        let next_children = toolbar_children(self.actions);
        // Keyed by verb: a selection change that shrinks or grows the enabled
        // set relocates a surviving item's live widget (and its press state)
        // rather than rebuilding whatever now sits at its index.
        crate::authoring::rebuild_children::<(), ToolbarChild>(
            &prev_children,
            &next_children,
            &mut element.items,
            ctx,
            |c| &c.view,
            |c| Some(ChildKey::new(c.action)),
        )
    }

    fn teardown(&self, element: &mut SelectionToolbarWidget, ctx: &mut BuildCtx<'_>) {
        let children = toolbar_children(self.actions);
        for (child, pod) in children.iter().zip(element.items.iter_mut()) {
            crate::authoring::teardown_child(&child.view, pod, ctx);
        }
    }
}

/// The retained pill widget: the pill's own chrome (painted here, theme-first
/// — see [`ToolbarChrome`]) around a left-to-right, no-gap row of item pods.
struct SelectionToolbarWidget {
    items: Vec<ChildPod>,
}

impl Widget for SelectionToolbarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Sized to content (module docs): each item gets a loose probe up to
        // the incoming max (the window, under the overlay pod's own loose
        // constraint), and the pill hugs their sum/tallest exactly.
        let item_bc = BoxConstraints::loose(bc.max());
        let mut x = 0.0;
        let mut height: f64 = 0.0;
        for pod in &mut self.items {
            let size = pod.layout_child(ctx, &item_bc);
            pod.set_origin(Point::new(x, 0.0));
            x += size.width;
            height = height.max(size.height);
        }
        bc.constrain(Size::new(x, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = ToolbarChrome::resolve(theme);
        let origin = ctx.origin();
        let size = ctx.size();
        if size.width > 0.0 && size.height > 0.0 {
            scene.fill_rounded_rect(origin, size, RADIUS, chrome.fill);
            // Inset by half the stroke width so the hairline paints fully
            // inside the pill's own bounds (mirrors `crate::button`'s
            // identical border discipline).
            let half = BORDER_WIDTH / 2.0;
            let rr = RoundedRect::new(
                half,
                half,
                size.width - half,
                size.height - half,
                (RADIUS - half).max(0.0),
            );
            let path = rr.to_path(BORDER_TOLERANCE);
            scene.stroke_path(origin, &path, BORDER_WIDTH, &Brush::Solid(chrome.border));
        }
        for pod in &mut self.items {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event(&mut self.items, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        for pod in &self.items {
            pod.semantics_child(ctx);
        }
    }

    crate::authoring::visit_children!(items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use frust_core::{FrameTime, PointerButton, PointerEvent, RenderRoot};
    use kurbo::Rect;

    use crate::overlay::{OverlayAnchor, OverlayPlacement, OverlaySlot};
    use crate::test_support::RecordingScene;

    const WINDOW: Size = Size::new(400.0, 600.0);

    fn request(actions: SelectionToolbarActions) -> SelectionToolbarRequest {
        SelectionToolbarRequest {
            anchor: Rect::new(150.0, 100.0, 250.0, 120.0),
            actions,
            // A builder only ever runs for a bar that is wanted; the flag is
            // the platform route's, and this view ignores it.
            present_menu: true,
        }
    }

    // --- pure widget-level tests (no RenderRoot needed) --------------------

    fn build_widget(actions: SelectionToolbarActions) -> SelectionToolbarWidget {
        let view = SelectionToolbarView { actions };
        let mut next_id = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut next_id))
    }

    #[test]
    fn only_enabled_actions_render() {
        let w = build_widget(SelectionToolbarActions {
            cut: false,
            copy: true,
            paste: true,
            select_all: false,
        });
        assert_eq!(w.items.len(), 2, "only Copy and Paste are enabled");
    }

    #[test]
    fn every_action_renders_one_item_each_in_display_order() {
        let w = build_widget(SelectionToolbarActions {
            cut: true,
            copy: true,
            paste: true,
            select_all: true,
        });
        assert_eq!(w.items.len(), 4);
    }

    #[test]
    fn no_enabled_actions_renders_nothing() {
        let w = build_widget(SelectionToolbarActions::default());
        assert!(w.items.is_empty());
    }

    #[test]
    fn chrome_resolves_theme_first_with_a_fallback_when_unthemed() {
        let unthemed = ToolbarChrome::resolve(None);
        assert_eq!(unthemed.fill, FALLBACK_FILL);
        assert_eq!(unthemed.border, FALLBACK_BORDER);

        let theme = Theme::neutral();
        let scheme = theme.scheme();
        let themed = ToolbarChrome::resolve(Some(&theme));
        assert_eq!(themed.fill, scheme.surface_container);
        assert_eq!(themed.border, scheme.outline_variant);
        assert_ne!(
            themed.fill, unthemed.fill,
            "fixture sanity: the neutral theme's role must actually differ \
             from the unthemed fallback, or this test proves nothing"
        );
    }

    // --- hosted through a real root, via an OverlaySlot<()> ----------------
    // Mirrors `crate::overlay`'s own `ToolbarHost`/`unit_harness` test shape:
    // a small owner that mounts `selection_toolbar`'s output through an
    // `OverlaySlot<()>`, exactly the way a text field hosts its own pod.

    type Log = Rc<RefCell<Vec<EditCommand>>>;

    struct Host {
        actions: SelectionToolbarActions,
        log: Log,
    }

    struct HostWidget {
        slot: OverlaySlot<()>,
        view: Option<AnyView<()>>,
        actions: SelectionToolbarActions,
        log: Log,
    }

    impl Host {
        fn pod(&self) -> AnyView<()> {
            selection_toolbar(&request(self.actions))
        }
    }

    impl View<()> for Host {
        type Element = HostWidget;
        fn build(&self, ctx: &mut BuildCtx<'_>) -> HostWidget {
            let mut slot = OverlaySlot::new();
            slot.set_placement(OverlayPlacement::default());
            slot.set_anchor(OverlayAnchor::Rect(request(self.actions).anchor));
            let view = self.pod();
            slot.rebuild(None, Some(&view), ctx);
            HostWidget {
                slot,
                view: Some(view),
                actions: self.actions,
                log: Rc::clone(&self.log),
            }
        }
        fn rebuild(
            &self,
            _prev: &Self,
            element: &mut HostWidget,
            ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            element.actions = self.actions;
            element.log = Rc::clone(&self.log);
            let view = self.pod();
            let flags = element
                .slot
                .rebuild(element.view.as_ref(), Some(&view), ctx);
            element.view = Some(view);
            flags
        }
    }

    impl Widget for HostWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            self.slot.layout(ctx);
            bc.constrain(Size::new(300.0, 300.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            let size = ctx.size();
            self.slot.paint(ctx, size);
        }
        fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            let mut pod_state = ();
            let Some(result) = self.slot.event(ctx, event, &mut pod_state) else {
                return EventResult::Ignored;
            };
            // Drained the instant the forward returns, per
            // `EventCtx::take_edit_commands`'s own contract.
            for command in ctx.take_edit_commands() {
                self.log.borrow_mut().push(command);
            }
            result
        }
    }

    /// Drive a single-item pill through a real root, laid out and painted
    /// once. The single anchor `request()` uses (width 100, centered at
    /// x=150+50=200) keeps every click test below geometry-agnostic: under
    /// `OverlayAlign::Center` (the default), the placed pill's own horizontal
    /// center always lands exactly on the anchor's horizontal center,
    /// regardless of the pill's actual (text-shaping-dependent) width — see
    /// `crate::overlay::place`'s `align_start` — so a click at that x is
    /// always inside the pill without this test needing to know its width.
    fn harness(actions: SelectionToolbarActions) -> (RenderRoot<(), Host>, Log) {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut root: RenderRoot<(), Host> = RenderRoot::new();
        let captured = Rc::clone(&log);
        let mut app_logic = move |_: &mut ()| Host {
            actions,
            log: Rc::clone(&captured),
        };
        let mut state = ();
        root.rebuild(&mut app_logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn std::any::Any);
        root.paint(&mut RecordingScene::default(), FrameTime::from_nanos(0));
        (root, log)
    }

    /// The anchor's horizontal center (see `harness`'s doc comment) and a
    /// small fixed offset below its bottom edge plus the default anchor gap —
    /// safely inside a pill at least [`MIN_HEIGHT`] tall regardless of its
    /// (text-shaping-dependent) width.
    fn click_point() -> Point {
        Point::new(200.0, 100.0 + 20.0 + crate::overlay::DEFAULT_OFFSET + 6.0)
    }

    fn pointer(phase: PointerPhase, p: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: p,
            button: PointerButton::Primary,
        })
    }

    #[test]
    fn a_tap_on_copy_enqueues_edit_command_copy() {
        let (mut root, log) = harness(SelectionToolbarActions {
            copy: true,
            ..Default::default()
        });
        let mut state = ();
        let p = click_point();
        root.event(&mut state, &pointer(PointerPhase::Down, p));
        root.event(&mut state, &pointer(PointerPhase::Up, p));
        assert_eq!(
            log.borrow().clone(),
            vec![EditCommand::Copy],
            "an up-inside tap on the sole Copy item must dispatch EditCommand::Copy"
        );
    }

    #[test]
    fn a_tap_on_paste_sets_take_paste_request() {
        let (mut root, _log) = harness(SelectionToolbarActions {
            paste: true,
            ..Default::default()
        });
        let mut state = ();
        assert!(!root.take_paste_request(), "nothing asked yet");
        let p = click_point();
        root.event(&mut state, &pointer(PointerPhase::Down, p));
        root.event(&mut state, &pointer(PointerPhase::Up, p));
        assert!(
            root.take_paste_request(),
            "an up-inside tap on the sole Paste item must raise the paste request"
        );
        assert!(
            !root.take_paste_request(),
            "draining the request must clear it"
        );
    }

    #[test]
    fn a_tap_that_moves_out_before_release_fires_nothing() {
        let (mut root, log) = harness(SelectionToolbarActions {
            copy: true,
            ..Default::default()
        });
        let mut state = ();
        let p = click_point();
        root.event(&mut state, &pointer(PointerPhase::Down, p));
        // Released far outside the pill entirely.
        root.event(&mut state, &pointer(PointerPhase::Up, Point::new(0.0, 0.0)));
        assert!(
            log.borrow().is_empty(),
            "an up-outside release must not dispatch anything"
        );
        assert!(!root.take_paste_request());
    }

    #[test]
    fn semantics_exposes_one_button_node_per_enabled_action_with_its_label() {
        fn logic(_s: &mut ()) -> AnyView<()> {
            selection_toolbar(&request(SelectionToolbarActions {
                cut: true,
                copy: true,
                paste: false,
                select_all: true,
            }))
        }
        let mut root: RenderRoot<(), AnyView<()>> = RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(WINDOW, &mut tcx as &mut dyn std::any::Any);
        let update = root.semantics();
        let buttons: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .collect();
        assert_eq!(buttons.len(), 3, "one node per enabled verb, not per glyph");
        let labels: Vec<_> = buttons.iter().filter_map(|(_, n)| n.label()).collect();
        assert_eq!(labels, vec!["Cut", "Copy", "Select all"]);
        for (_, node) in &buttons {
            assert!(node.supports_action(Action::Click));
        }
    }
}
