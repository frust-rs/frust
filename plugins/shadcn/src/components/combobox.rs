//! `combobox`: a select whose list is filtered by a search field.
//!
//! Source: `apps/v4/registry/new-york-v4/ui/combobox.tsx` plus the registry's own
//! combobox example (shadcn/ui v4, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17). The
//! canonical shadcn combobox is a **`Popover` wrapping a `Command`**: a
//! `role="combobox"` button trigger showing the chosen value with a `size-4
//! opacity-50` chevron, and a `w-[200px] p-0` popover panel holding the command
//! palette's search input, its filtered list, its empty state and its
//! check-marked items.
//!
//! # Shape of the port
//!
//! It is that composition, verbatim: the trigger is [`crate::select`]'s (the same
//! bordered `h-9` control, the same shared field chrome), and the panel is
//! [`crate::command`]'s palette inside [`crate::popover`]'s shared panel at
//! `p-0`. So the substring filter, the empty row, the group headings, the
//! arrow/Enter keyboard model and the input's own chrome are all the command
//! palette's — this module adds the popover mount, the fixed width, and nothing
//! else.
//!
//! Query and selection are **controlled**, like every other input in the catalog:
//! the query comes in as a prop and each edit is reported through
//! `on_query_change`; `on_select` reports the item's index into the *unfiltered*
//! list.
//!
//! # v1 scope
//!
//! * **Single select.** The multi-select variant (chips inside the trigger, a
//!   clear button) is not ported; `on_select` reports one index and the app
//!   decides what to do with it.
//! * **Fixed panel width.** Upstream pins `w-[200px]` on the popover content
//!   rather than matching the trigger, and so does this
//!   ([`COMBOBOX_WIDTH`], overridable with [`ComboboxView::width`]) — unlike
//!   [`crate::select`], whose panel takes the trigger's width as a floor.
//! * **No auto-focus on appear.** The framework has no focus-on-mount hook, so
//!   the search field needs one click before it accepts typing — the same limit
//!   [`crate::command`] documents.

use std::cell::RefCell;
use std::rc::Rc;

use frust::authoring::{BuildCtx, ChangeFlags, View};

use crate::components::command::{CommandItem, CommandView, command, command_item};
use crate::components::popover::{PanelHandle, PanelStyle, panel};
use crate::components::select::{SelectTriggerView, select_option, select_trigger};
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::style;

/// `w-[200px]` — the combobox panel's width, in logical px.
pub const COMBOBOX_WIDTH: f64 = 200.0;

/// One entry in a [`combobox`] list — the command palette's item, since the
/// panel *is* a command palette (see the [module docs](self)).
pub type ComboboxItem = CommandItem;

/// Create a combobox entry labelled `label`.
pub fn combobox_item(label: impl Into<String>) -> ComboboxItem {
    command_item(label)
}

impl PanelStyle {
    /// `w-[200px] p-0 shadow-md` — the combobox panel's chrome. The palette
    /// inside it paints its own padding, so the panel adds none.
    pub(crate) fn combobox(width: f64) -> Self {
        PanelStyle {
            pad_x: 0.0,
            pad_y: 0.0,
            width: Some(width),
            min_width: 0.0,
            anchor: None,
            shadow: style::SHADOW_MD,
            entrance: true,
            open: true,
        }
    }
}

/// Create the combobox's trigger: [`crate::select`]'s control showing
/// `selected_label` (or its placeholder while nothing is chosen), capturing its
/// own rect into `anchor`.
pub fn combobox_trigger<State: 'static>(
    anchor: &OverlayAnchor,
    selected_label: Option<String>,
) -> SelectTriggerView<State> {
    let selected = selected_label.is_some().then_some(0);
    let options = selected_label
        .map(|l| vec![select_option(l)])
        .unwrap_or_default();
    select_trigger(anchor, options, selected)
}

/// A declarative shadcn combobox. See [`combobox`].
pub struct ComboboxView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    style: PanelHandle,
    placement: OverlayPlacement,
}

/// Build a combobox panel over `items`, filtered by the controlled `query`.
///
/// `on_query_change(state, text)` reports each edit of the search field;
/// `on_select(state, index)` reports an activation, with `index` counted into
/// `items` as given (not into the filtered rows).
///
/// Mount it while the app's own open flag is set, or keep it mounted and hand
/// the flag to [`ComboboxView::open`] for an exit ramp (see [`crate::popover`]
/// for both mount contracts).
pub fn combobox<State: 'static, F, G>(
    items: Vec<ComboboxItem>,
    query: impl Into<String>,
    on_query_change: F,
    on_select: G,
) -> ComboboxView<State>
where
    F: Fn(&mut State, String) + 'static,
    G: Fn(&mut State, usize) + 'static,
{
    let style: PanelHandle = Rc::new(RefCell::new(PanelStyle::combobox(COMBOBOX_WIDTH)));
    let palette: CommandView<State> = command(items, query, on_query_change, on_select)
        .placeholder("Search...")
        .empty("No results found.");
    // shadcn's combobox popover aligns to the trigger's leading edge.
    let placement = OverlayPlacement::default().align(OverlayAlign::Start);
    ComboboxView {
        inner: anchored(panel(palette, style.clone())).placement(placement),
        style,
        placement,
    }
}

impl<State: 'static> ComboboxView<State> {
    /// Anchor the panel to the trigger's captured rect.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Set the side the panel opens on (default `bottom`).
    pub fn side(mut self, side: OverlaySide) -> Self {
        self.placement.side = side;
        self.apply_placement()
    }

    /// Set the cross-axis alignment (default `start`).
    pub fn align(mut self, align: OverlayAlign) -> Self {
        self.placement.align = align;
        self.apply_placement()
    }

    /// Set the gap between trigger and panel (`sideOffset`, default `4`).
    pub fn offset(mut self, offset: f64) -> Self {
        self.placement.offset = offset;
        self.apply_placement()
    }

    /// Override the panel's [`COMBOBOX_WIDTH`].
    pub fn width(self, width: f64) -> Self {
        self.style.borrow_mut().width = Some(width);
        self
    }

    /// Hand a **kept-mounted** combobox the app's open flag, so closing it plays
    /// the popover panel's exit ramp instead of vanishing (see
    /// [`crate::popover`]).
    ///
    /// The default is `true`: a mounted panel is an open one.
    pub fn open(mut self, open: bool) -> Self {
        self.style.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the open-change callback: a press outside the panel or a focus-routed
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, WINDOW, escape, ft_ms, light, pointer};
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BoxConstraints, InputEvent, LayoutCtx, Point, PointerPhase, Rect, Size, Widget, any,
    };
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct AppState {
        open: bool,
        opens: Vec<bool>,
        query: String,
        value: Option<usize>,
        commits: Vec<usize>,
    }

    fn items() -> Vec<ComboboxItem> {
        vec![
            combobox_item("Next.js"),
            combobox_item("SvelteKit"),
            combobox_item("Nuxt.js"),
            combobox_item("Remix"),
        ]
    }

    /// `duration-200`, the shared ramp the panel exits over.
    const RAMP_MS: f64 = 200.0;

    struct Harness {
        root: RenderRoot<AppState, frust::StackView<AppState>>,
        state: AppState,
        tcx: TextContext,
        anchor: OverlayAnchor,
        /// Whether the panel is kept mounted and handed the flag (the mount an
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
                let label = state
                    .value
                    .and_then(|i| items().get(i).map(|item| item.label().to_string()));
                let trigger = combobox_trigger(&anchor, label)
                    .placeholder("Select framework...")
                    .open(state.open)
                    .on_open_change(|s: &mut AppState, open| {
                        s.opens.push(open);
                        s.open = open;
                    });
                let mut children = vec![any(trigger)];
                if kept || state.open {
                    children.push(any(combobox(
                        items(),
                        state.query.clone(),
                        |s: &mut AppState, text| s.query = text,
                        |s: &mut AppState, index| {
                            s.commits.push(index);
                            s.value = Some(index);
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

        /// Whether the palette's `bg-popover` panel was drawn.
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

        /// The centre of the list's first visible row, under the search input.
        fn first_row(&self) -> Point {
            let trigger = self.anchor.rect();
            Point::new(
                trigger.x0 + 40.0,
                trigger.y1
                    + crate::overlay::SIDE_OFFSET
                    + style::HEIGHT_DEFAULT
                    + style::BORDER_WIDTH
                    + 4.0
                    + 14.0,
            )
        }
    }

    #[test]
    fn the_trigger_toggles_the_panel_and_a_press_outside_closes_it() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        assert_eq!(h.state.opens, vec![true]);
        h.event(pointer(PointerPhase::Down, 380.0, 580.0));
        assert_eq!(h.state.opens, vec![true, false]);
        assert!(!h.state.open);
    }

    #[test]
    fn escape_closes_the_panel_once_a_press_has_focused_it() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        let row = h.first_row();
        h.event(pointer(PointerPhase::Down, row.x, row.y));
        h.event(escape());
        assert!(!h.state.opens.last().unwrap(), "the last report is a close");
    }

    #[test]
    fn clicking_a_row_commits_its_unfiltered_index_and_closes() {
        let mut h = Harness::new();
        h.click(40.0, 18.0);
        let row = h.first_row();
        h.click(row.x, row.y);
        assert_eq!(h.state.commits, vec![0], "`Next.js`");
        assert_eq!(h.state.value, Some(0));
        assert!(!h.state.open, "a commit closes the panel");
    }

    #[test]
    fn the_query_filters_the_rows_so_the_first_one_is_the_match() {
        let mut h = Harness::new();
        h.state.query = "remix".to_string();
        h.click(40.0, 18.0);
        let row = h.first_row();
        h.click(row.x, row.y);
        assert_eq!(
            h.state.commits,
            vec![3],
            "the substring filter leaves `Remix` alone at the top"
        );
    }

    #[test]
    fn a_kept_mounted_panel_paints_out_its_exit_and_takes_no_input_meanwhile() {
        let mut h = Harness::kept_mounted();
        h.click(40.0, 18.0);
        h.paint_at(RAMP_MS * 2.0);
        assert!(Harness::panel_painted(&h.paint_at(h.clock)));

        let row = h.first_row();
        h.click(row.x, row.y);
        assert_eq!(h.state.commits, vec![0], "the commit reports at once");
        assert!(!h.state.open);

        let start = h.clock;
        let mid = h.paint_at(start + RAMP_MS / 2.0);
        assert!(Harness::panel_painted(&mid), "still on screen");
        assert!(mid.layers[0] > 0.0 && mid.layers[0] < 1.0);

        let before = h.state.commits.len();
        let outcome = h
            .root
            .event(&mut h.state, &pointer(PointerPhase::Down, row.x, row.y));
        assert!(outcome.handled, "a closing panel swallows a press that lands on it");
        assert_eq!(h.state.commits.len(), before);

        assert!(
            !Harness::panel_painted(&h.paint_at(start + RAMP_MS * 2.0)),
            "gone at settle"
        );
    }

    #[test]
    fn the_panel_is_the_fixed_upstream_width_and_overridable() {
        let anchor = OverlayAnchor::new();
        anchor.set(Rect::new(0.0, 0.0, 320.0, style::HEIGHT_DEFAULT));
        let measure = |view: ComboboxView<AppState>| {
            let mut counter = 0u64;
            let mut w = View::<AppState>::build(&view, &mut BuildCtx::new(&mut counter));
            let mut tcx = TextContext::new();
            let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
            w.layout(&mut lctx, &BoxConstraints::tight(WINDOW));
            w.content_rect().size()
        };
        let default: Size = measure(
            combobox(
                items(),
                "",
                |_s: &mut AppState, _t| {},
                |_s: &mut AppState, _i| {},
            )
            .anchor(&anchor),
        );
        assert_eq!(
            default.width, COMBOBOX_WIDTH,
            "w-[200px], not the trigger's"
        );
        let wide: Size = measure(
            combobox(
                items(),
                "",
                |_s: &mut AppState, _t| {},
                |_s: &mut AppState, _i| {},
            )
            .anchor(&anchor)
            .width(320.0),
        );
        assert_eq!(wide.width, 320.0);
    }
}
