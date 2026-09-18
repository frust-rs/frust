//! Ports beUI's `expandable-tabs` composed block —
//! `components/motion/expandable-tabs.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `expandable-tabs`: *"Icon tab bar where the active tab expands to a
//! labelled pill, with a panel above that morphs height and slides content
//! direction-aware on switch."*
//!
//! | upstream | here |
//! |---|---|
//! | root `rounded-[26px] border border-border bg-card overflow-hidden` | the shell, [`EXPANDABLE_TABS_RADIUS`] |
//! | `BAR_H = 52`, `TAB_W = 32`, `BAR_X = 16`, `BAR_GAP = 4` | [`EXPANDABLE_TABS_BAR_HEIGHT`], [`EXPANDABLE_TABS_TAB_WIDTH`], [`EXPANDABLE_TABS_BAR_INSET`], [`EXPANDABLE_TABS_BAR_GAP`] |
//! | `ROOT_BORDER = 2`, `ICON_W = 16`, `PANEL_DOCK_GAP = 4` | [`EXPANDABLE_TABS_BORDER`], [`EXPANDABLE_TABS_ICON_BOX`], [`EXPANDABLE_TABS_PANEL_DOCK_GAP`] |
//! | `ACTIVE_LEFT_PAD = 10`, `ACTIVE_RIGHT_PAD = 16`, `LABEL_GAP = 7` | [`EXPANDABLE_TABS_ACTIVE_PAD_LEFT`], [`EXPANDABLE_TABS_ACTIVE_PAD_RIGHT`], [`EXPANDABLE_TABS_LABEL_GAP`] |
//! | tab `h-9 rounded-[18px] text-sm font-medium` | [`EXPANDABLE_TABS_TAB_HEIGHT`], [`EXPANDABLE_TABS_TAB_RADIUS`], [`crate::style::TEXT_SM`] |
//! | active pill `absolute inset-0 rounded-[18px] bg-foreground/10` | [`EXPANDABLE_TABS_PILL_ALPHA`] |
//! | panel container `px-2 pt-2`, `bottom: BAR_H + PANEL_DOCK_GAP` | [`EXPANDABLE_TABS_PANEL_PADDING`] over the reserved dock band |
//! | `SHELL_SPRING {duration: 0.58, bounce: 0.06}` | [`EXPANDABLE_TABS_SHELL_SPRING`] (converted) |
//! | `TAB_CHANGE_SPRING {duration: 0.46, bounce: 0.04}` | [`EXPANDABLE_TABS_TAB_SPRING`] (converted) |
//! | `LABEL_OPEN {duration: 0.38, bounce: 0.03}` | [`EXPANDABLE_TABS_LABEL_OPEN`] (converted) |
//! | `LABEL_CLOSE {duration: 0.16, ease: EASE_OUT}` | [`EXPANDABLE_TABS_LABEL_CLOSE`] |
//! | `CONTENT_SPRING {duration: 0.46, bounce: 0.08}` | [`EXPANDABLE_TABS_CONTENT_SPRING`] (converted) |
//! | content `enter {y:-8, scale:0.98, opacity:0}` | [`EXPANDABLE_TABS_CONTENT_ENTER_LIFT`], [`EXPANDABLE_TABS_CONTENT_SCALE`] |
//! | content `exit {y:-6, scale:0.98, opacity:0, 0.08s}` | [`EXPANDABLE_TABS_CONTENT_EXIT_LIFT`], [`EXPANDABLE_TABS_CONTENT_EXIT`] |
//!
//! # Premise correction: the block is a shell, not just a tab bar
//!
//! The porting card describes this slug as "icon tabs where the active tab
//! expands to show its label (width spring per tab, label fade/slide)". That is
//! half of it. `expandable-tabs.tsx` is a whole **shell**: a rounded card whose
//! *own* width and height spring between a closed bar-only box and an open box
//! sized to its content panel, with the icon bar docked along the bottom edge
//! and the active tab's panel unfurling above it. The label reveal is one of
//! four concurrent animations, not the component.
//!
//! Two consequences of reading the source rather than the summary are load
//! bearing here:
//!
//! - **The open size does not change between tabs.** Upstream measures a hidden
//!   sizer that stacks *every* item's content in one CSS grid cell (`col-start-1
//!   row-start-1`), so the open box is the max over all panels and switching
//!   tabs moves no shell edge. This port measures every panel and takes the same
//!   max, which is why a tab switch here is a content crossfade against a still
//!   shell rather than a second resize.
//! - **Pressing the active tab closes the shell.** `onClick={() => setActive(
//!   isActive ? null : item.id)}` — the value is `Option`-shaped, and `None` is
//!   the closed, bar-only state rather than an error case.
//!
//! # Motion's `{ duration, bounce }` springs, converted
//!
//! All four of this file's springs are authored in Motion's perceptual form.
//! They are converted by the identity
//! [`crate::components::bouncy_accordion`] establishes for the same reason:
//!
//! ```text
//! ζ  = 1 − bounce
//! ω₀ = −ln(ε) / (ζ · duration)      with ε = 1e-3, this crate's settle epsilon
//! mass = 1, stiffness = ω₀², damping = 2 · ζ · ω₀
//! ```
//!
//! so [`Ramp::settle`](crate::motion::Ramp::settle) of each converted spring is
//! upstream's own stated duration and the bounce is preserved exactly. A test
//! pins the settle half.
//!
//! # A layout animation, timed at paint and applied at layout
//!
//! Three of the four animated quantities — the shell box, each tab's width and
//! each label's reveal — resize the widget, and `layout` carries no clock. This
//! port takes the answer [`crate::blocks::expandable_action_bar`] documents:
//! `layout` **retargets** every `Lane` (it is the pass that knows the measured
//! label widths and panel boxes) and reads their current values; `paint`
//! advances them and calls `PaintCtx::request_layout` while any is still moving.
//! The cost is the documented one-frame lag, not a second clock.
//!
//! # Controlled, `Option`-valued
//!
//! `value` is a prop and the widget never writes it: a press, `Escape` and
//! keyboard activation all *report* the requested value through
//! `on_value_change`, so the app stays the single source of truth — the
//! convention every controlled component in this catalog keeps.
//!
//! # Degradations against the web original
//!
//! - **No blur.** Every upstream content stage also animates
//!   `filter: blur(4px) → 0`. `PaintScene` publishes no blur filter, so the
//!   fade, the lift and the scale carry the entrance and the exit.
//! - **No outside-press dismissal.** Upstream closes on a `pointerdown`
//!   anywhere outside the root, through a document listener. This is an inline
//!   widget, not an [`crate::overlay`] host, so it never sees a press that
//!   missed it; `Escape` and pressing the open tab both still close it. The
//!   same gap [`crate::blocks::expandable_action_bar`] records.
//! - **The keyboard moves a widget-local highlight, not focus.** Upstream tabs
//!   between real `<button>`s; the whole block is one widget here, so the arrows
//!   move a roving highlight and `Enter`/`Space` activates it — the call
//!   [`crate::components::context_menu`] and `expandable_action_bar` both make,
//!   with the same visible result. Upstream binds no arrow keys at all, so this
//!   is an addition rather than a substitution.
//! - **Every panel is measured, only the visible ones are painted.** Upstream's
//!   hidden sizer really does render every panel on every commit (that is how
//!   the open box stays stable), so every panel is measured here too; but only a
//!   visible one is painted, routed to or published to semantics — the
//!   input-parity carve-out [`crate::components::tabs`] documents for the same
//!   shape. A closed panel therefore costs a layout pass it would not cost in a
//!   mount-on-open port, which is the price of the stable open box.
//! - **`justify-between` is honoured, `ResizeObserver` is not.** The bar spreads
//!   its tabs across whatever width the shell currently has, as upstream's
//!   `justify-between` does. There is no observer: a label whose text changes
//!   re-measures on the rebuild that changed it.
//! - **No direction-aware slide.** The registry blurb promises content that
//!   "slides direction-aware on switch"; the file itself has no such thing —
//!   `CONTENT_VARIANTS` is one direction-free `y`/`scale`/`blur` triple. The
//!   port follows the source, not the blurb.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any,
    build_child, erase_callback_arg, rebuild_children, route_event_single, teardown_child,
    text::TextStyle, visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::components::popover::lerp;
use crate::motion::{Presence, PresencePhase, Ramp};
use crate::press::{Lane, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// The docked icon bar's height, in logical px (`BAR_H = 52` — `p-2` plus an
/// `h-9` button). Fixed upstream so the panel's bottom reserve is static and
/// the open height is right on the first frame.
pub const EXPANDABLE_TABS_BAR_HEIGHT: f64 = 52.0;

/// A collapsed tab's width, in logical px (`TAB_W = 32`).
pub const EXPANDABLE_TABS_TAB_WIDTH: f64 = 32.0;

/// The bar's total horizontal padding, in logical px (`BAR_X = 16`, i.e. `p-2`
/// on both sides).
pub const EXPANDABLE_TABS_BAR_INSET: f64 = 16.0;

/// The minimum gap between two tabs, in logical px (`BAR_GAP = 4`, `gap-1`).
pub const EXPANDABLE_TABS_BAR_GAP: f64 = 4.0;

/// The shell's total border thickness, in logical px (`ROOT_BORDER = 2`, a 1px
/// border on each side).
pub const EXPANDABLE_TABS_BORDER: f64 = 2.0;

/// An icon's box, in logical px (`ICON_W = 16`).
pub const EXPANDABLE_TABS_ICON_BOX: f64 = 16.0;

/// The active tab's left padding, in logical px (`ACTIVE_LEFT_PAD = 10`,
/// `pl-2.5`).
pub const EXPANDABLE_TABS_ACTIVE_PAD_LEFT: f64 = 10.0;

/// The active tab's right padding, in logical px (`ACTIVE_RIGHT_PAD = 16`,
/// `pr-4`).
pub const EXPANDABLE_TABS_ACTIVE_PAD_RIGHT: f64 = 16.0;

/// The gap between an icon and its revealed label, in logical px
/// (`LABEL_GAP = 7`, animated as the label's `marginLeft`).
pub const EXPANDABLE_TABS_LABEL_GAP: f64 = 7.0;

/// The clearance between the content panel and the docked bar, in logical px
/// (`PANEL_DOCK_GAP = 4`).
pub const EXPANDABLE_TABS_PANEL_DOCK_GAP: f64 = 4.0;

/// The content panel's own padding, in logical px (`px-2 pt-2`).
pub const EXPANDABLE_TABS_PANEL_PADDING: f64 = 8.0;

/// The shell's corner radius, in logical px (`rounded-[26px]`).
pub const EXPANDABLE_TABS_RADIUS: f64 = 26.0;

/// A tab's corner radius, in logical px (`rounded-[18px]`).
pub const EXPANDABLE_TABS_TAB_RADIUS: f64 = 18.0;

/// A tab's height, in logical px (`h-9`).
pub const EXPANDABLE_TABS_TAB_HEIGHT: f64 = 36.0;

/// The active tab's pill wash (`bg-foreground/10`).
pub const EXPANDABLE_TABS_PILL_ALPHA: f32 = 0.10;

// ---- Motion ----------------------------------------------------------------

/// `SHELL_SPRING = { type: "spring", duration: 0.58, bounce: 0.06 }`, converted
/// (see the [module docs](self)). Drives the shell's own width and height.
pub const EXPANDABLE_TABS_SHELL_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 160.53,
    damping: 23.82,
};

/// `TAB_CHANGE_SPRING = { type: "spring", duration: 0.46, bounce: 0.04 }`,
/// converted. Drives each tab's box width.
pub const EXPANDABLE_TABS_TAB_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 244.69,
    damping: 30.03,
};

/// `LABEL_OPEN = { type: "spring", duration: 0.38, bounce: 0.03 }`, converted —
/// the label's reveal on the way **in**.
pub const EXPANDABLE_TABS_LABEL_OPEN: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 351.21,
    damping: 36.36,
};

/// `LABEL_CLOSE = { duration: 0.16, ease: EASE_OUT }` — the label's reveal on
/// the way **out**, a short tween rather than the entrance spring. The
/// asymmetry is upstream's, and is what `Lane::retarget_with` exists for.
pub const EXPANDABLE_TABS_LABEL_CLOSE: Ramp = Ramp::eased(Duration::from_millis(160), EASE_OUT);

/// `CONTENT_SPRING = { type: "spring", duration: 0.46, bounce: 0.08 }`,
/// converted — the panel's entrance.
pub const EXPANDABLE_TABS_CONTENT_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 266.43,
    damping: 30.03,
};

/// The panel's exit: `{ duration: 0.08, ease: EASE_OUT }` — fast, so the
/// outgoing panel is gone before the shrinking shell can clip it.
pub const EXPANDABLE_TABS_CONTENT_EXIT: Ramp = Ramp::eased(Duration::from_millis(80), EASE_OUT);

/// How far above its resting place an entering panel starts, in logical px
/// (`enter: { y: -8 }`).
pub const EXPANDABLE_TABS_CONTENT_ENTER_LIFT: f64 = 8.0;

/// How far above its resting place a leaving panel ends, in logical px
/// (`exit: { y: -6 }`).
pub const EXPANDABLE_TABS_CONTENT_EXIT_LIFT: f64 = 6.0;

/// The scale an entering/leaving panel is staged from (`scale: 0.98`).
pub const EXPANDABLE_TABS_CONTENT_SCALE: f64 = 0.98;

// ---- The view --------------------------------------------------------------

/// One tab: its identity, its accessible label, its icon view, and the panel
/// shown above the bar while it is open.
pub struct ExpandableTabsItem<State: 'static> {
    id: String,
    label: String,
    icon: AnyView<State>,
    content: AnyView<State>,
}

/// A tab identified by `id`, named `label`, drawing `icon` and disclosing
/// `content`.
///
/// The icon is a **view**, the child protocol [`crate::components::dock`]
/// established for the same reason: the catalog ships no icon vocabulary, so a
/// caller supplies its own icon view.
pub fn expandable_tabs_item<State: 'static, I: View<State>, C: View<State>>(
    id: impl Into<String>,
    label: impl Into<String>,
    icon: I,
    content: C,
) -> ExpandableTabsItem<State> {
    ExpandableTabsItem {
        id: id.into(),
        label: label.into(),
        icon: any(icon),
        content: any(content),
    }
}

impl<State: 'static> ExpandableTabsItem<State> {
    /// The id this tab selects.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A view-held selection callback, erased on build.
type OnValueChange<State> = Rc<dyn Fn(&mut State, Option<String>)>;

/// A declarative beUI expandable tab shell. See the [module docs](self).
pub struct ExpandableTabsView<State: 'static> {
    value: Option<String>,
    items: Vec<ExpandableTabsItem<State>>,
    on_value_change: OnValueChange<State>,
}

/// Create an expandable tab shell whose open tab is the one whose id equals
/// `value` (`None` is the closed, bar-only state), reporting a requested value
/// through `on_value_change` — a **controlled** component.
pub fn expandable_tabs<State: 'static, F: Fn(&mut State, Option<String>) + 'static>(
    value: Option<String>,
    items: Vec<ExpandableTabsItem<State>>,
    on_value_change: F,
) -> ExpandableTabsView<State> {
    ExpandableTabsView {
        value,
        items,
        on_value_change: Rc::new(on_value_change),
    }
}

/// The resolved shell palette.
struct ExpandableTabsColors {
    /// The shell's fill (`bg-card`).
    surface: Color,
    /// Its hairline (`border-border`).
    border: Color,
    /// The open tab's ink, and the wash its pill is tinted from
    /// (`text-foreground` / `bg-foreground/10`).
    ink: Color,
    /// A closed tab's ink (`text-muted-foreground`).
    dim_ink: Color,
}

/// Resolve the palette, falling back to the vendored **light** table with no
/// theme threaded — the unthemed posture every component in this catalog takes.
fn resolve_colors(theme: Option<&Theme>) -> ExpandableTabsColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            ExpandableTabsColors {
                surface: s.surface_container,
                border: s.outline_variant,
                ink: s.on_surface,
                dim_ink: s.on_surface_variant,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            ExpandableTabsColors {
                surface: p.card,
                border: p.border,
                ink: p.foreground,
                dim_ink: p.muted_foreground,
            }
        }
    }
}

/// The tab-label style (`text-sm font-medium`), in the theme's `label_large`
/// family.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    themed_style(
        crate::text::label_style(style::TEXT_SM),
        ThemeTextType::LabelLarge,
        theme,
    )
}

/// One retained tab.
struct TabEntry {
    id: String,
    label: String,
    run: LabelRun,
    /// The label's natural width, from the last layout.
    label_width: f64,
    /// The tab's box width, springing between collapsed and labelled.
    width: Lane,
    /// The label's `0 → 1` reveal, driving its clip width, its `marginLeft` and
    /// its alpha together.
    reveal: Lane,
    /// The tab's left edge, resolved at layout.
    x: f64,
}

impl TabEntry {
    fn from_item<State: 'static>(item: &ExpandableTabsItem<State>) -> Self {
        TabEntry {
            id: item.id.clone(),
            label: item.label.clone(),
            run: LabelRun::new(item.label.clone()),
            label_width: 0.0,
            width: Lane::at_rest(
                Ramp::spring(EXPANDABLE_TABS_TAB_SPRING),
                EXPANDABLE_TABS_TAB_WIDTH,
            ),
            reveal: Lane::at_rest(Ramp::spring(EXPANDABLE_TABS_LABEL_OPEN), 0.0),
            x: 0.0,
        }
    }

    /// Adopt `item`'s text, reporting whether a re-measure is owed.
    fn sync<State: 'static>(&mut self, item: &ExpandableTabsItem<State>) -> bool {
        let mut changed = self.id != item.id;
        self.id = item.id.clone();
        if self.label != item.label {
            self.label = item.label.clone();
            self.run.set_content(item.label.clone());
            changed = true;
        }
        changed
    }

    /// The tab's box at `bar_top`, using the width its lane currently shows.
    fn rect(&self, bar_top: f64) -> Rect {
        Rect::from_origin_size(
            Point::new(self.x, bar_top),
            Size::new(self.width.value().max(0.0), EXPANDABLE_TABS_TAB_HEIGHT),
        )
    }

    /// The icon's left offset inside the tab: centred while collapsed, at
    /// `pl-2.5` once labelled, travelling with the reveal.
    fn icon_left(&self) -> f64 {
        lerp(
            (EXPANDABLE_TABS_TAB_WIDTH - EXPANDABLE_TABS_ICON_BOX) / 2.0,
            EXPANDABLE_TABS_ACTIVE_PAD_LEFT,
            self.reveal.value().clamp(0.0, 1.0),
        )
    }
}

impl<State: 'static> View<State> for ExpandableTabsView<State> {
    type Element = ExpandableTabsWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ExpandableTabsWidget {
        let mut widget = ExpandableTabsWidget {
            entries: self.items.iter().map(TabEntry::from_item).collect(),
            icons: self
                .items
                .iter()
                .map(|item| build_child(&item.icon, ctx))
                .collect(),
            panels: self
                .items
                .iter()
                .map(|item| build_child(&item.content, ctx))
                .collect(),
            presences: self.items.iter().map(|_| fresh_presence()).collect(),
            panel_sizes: self.items.iter().map(|_| Size::ZERO).collect(),
            value: self.value.clone(),
            focused: 0,
            hovered: None,
            captured: None,
            shell_w: Lane::at_rest(Ramp::spring(EXPANDABLE_TABS_SHELL_SPRING), 0.0),
            shell_h: Lane::at_rest(Ramp::spring(EXPANDABLE_TABS_SHELL_SPRING), 0.0),
            sized: false,
            closed_size: Size::ZERO,
            open_size: Size::ZERO,
            bar_top: 0.0,
            on_value_change: erase_callback_arg(&self.on_value_change),
        };
        widget.focused = widget.active().unwrap_or(0);
        widget.sync_presences();
        widget.settle_initial();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ExpandableTabsWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;

        if prev.items.len() != self.items.len() {
            element.entries = self.items.iter().map(TabEntry::from_item).collect();
            element.presences = self.items.iter().map(|_| fresh_presence()).collect();
            element.panel_sizes = self.items.iter().map(|_| Size::ZERO).collect();
            element.captured = None;
            element.hovered = None;
            element.sized = false;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.sync(item) {
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        }

        if prev.value != self.value {
            element.value = self.value.clone();
            if let Some(index) = element.active() {
                element.focused = index;
            }
            element.sync_presences();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.icons,
            ctx,
            |item: &ExpandableTabsItem<State>| &item.icon,
            |_| None::<ChildKey>,
        );
        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.panels,
            ctx,
            |item: &ExpandableTabsItem<State>| &item.content,
            |_| None::<ChildKey>,
        );
        element.focused = element.focused.min(element.entries.len().saturating_sub(1));
        flags
    }

    fn teardown(&self, element: &mut ExpandableTabsWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.icons.iter_mut()) {
            teardown_child(&item.icon, pod, ctx);
        }
        for (item, pod) in self.items.iter().zip(element.panels.iter_mut()) {
            teardown_child(&item.content, pod, ctx);
        }
    }
}

/// A panel's closed presence: the content spring in, the short tween out.
fn fresh_presence() -> Presence {
    Presence::new(
        Ramp::spring(EXPANDABLE_TABS_CONTENT_SPRING),
        EXPANDABLE_TABS_CONTENT_EXIT,
    )
}

/// The retained widget for an [`ExpandableTabsView`].
pub struct ExpandableTabsWidget {
    entries: Vec<TabEntry>,
    /// One icon pod per tab, always laid out (the bar is always visible).
    icons: Vec<ChildPod>,
    /// One panel pod per tab; only a visible one is laid out, painted, routed
    /// to or published.
    panels: Vec<ChildPod>,
    /// One presence per panel, open exactly for the active tab.
    presences: Vec<Presence>,
    /// Each panel's natural box, from the last layout that measured it.
    panel_sizes: Vec<Size>,
    /// The app-confirmed open tab (`None` is the closed shell).
    value: Option<String>,
    /// The tab the roving highlight sits on.
    focused: usize,
    /// The latched hovered tab, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The tab a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<usize>,
    /// The shell's own width, springing between closed and open.
    shell_w: Lane,
    /// The shell's own height, springing between closed and open.
    shell_h: Lane,
    /// Whether the shell lanes have ever been placed — a first layout lands on
    /// its box rather than growing into it.
    sized: bool,
    /// The bar-only box, from the last layout.
    closed_size: Size,
    /// The content box, from the last layout — the max over *every* panel, as
    /// upstream's stacked sizer grid measures.
    open_size: Size,
    /// The tab row's top edge inside the shell, from the last layout.
    bar_top: f64,
    on_value_change: ErasedArgCallback<Option<String>>,
}

impl ExpandableTabsWidget {
    /// The open tab's index, if any tab's id matches the value.
    fn active(&self) -> Option<usize> {
        let value = self.value.as_deref()?;
        self.entries.iter().position(|e| e.id == value)
    }

    /// Apply the confirmed value to every panel's presence.
    fn sync_presences(&mut self) {
        let active = self.active();
        for (index, presence) in self.presences.iter_mut().enumerate() {
            presence.set_open(active == Some(index));
        }
    }

    /// Drive whichever panel is open straight to `Present` —
    /// `<AnimatePresence initial={false}>`: a shell that mounts open is simply
    /// open, it does not play an entrance nobody asked for.
    fn settle_initial(&mut self) {
        let settled = FrameTime::from_nanos(
            Ramp::spring(EXPANDABLE_TABS_CONTENT_SPRING)
                .settle()
                .as_nanos() as u64
                + 1,
        );
        for presence in &mut self.presences {
            presence.advance(FrameTime::ZERO);
            presence.advance(settled);
        }
    }

    /// Whether panel `index` still has to be laid out and painted — true right
    /// through its exit ramp, which is what makes the exit visible.
    fn panel_visible(&self, index: usize) -> bool {
        self.presences.get(index).is_some_and(Presence::is_visible)
    }

    /// The labelled width tab `index` expands to (`getActiveTabWidth`), never
    /// narrower than the collapsed box.
    fn active_width(&self, index: usize) -> f64 {
        let label = self.entries[index].label_width;
        EXPANDABLE_TABS_TAB_WIDTH.max(
            EXPANDABLE_TABS_ACTIVE_PAD_LEFT
                + EXPANDABLE_TABS_ICON_BOX
                + EXPANDABLE_TABS_LABEL_GAP
                + label
                + EXPANDABLE_TABS_ACTIVE_PAD_RIGHT,
        )
    }

    /// The bar-only box: `items * TAB_W + gaps + BAR_X + ROOT_BORDER` by
    /// `BAR_H + ROOT_BORDER` (`closedSize`).
    fn closed_box(&self) -> Size {
        let count = self.entries.len();
        let width = count as f64 * EXPANDABLE_TABS_TAB_WIDTH
            + count.saturating_sub(1) as f64 * EXPANDABLE_TABS_BAR_GAP
            + EXPANDABLE_TABS_BAR_INSET
            + EXPANDABLE_TABS_BORDER;
        Size::new(width, EXPANDABLE_TABS_BAR_HEIGHT + EXPANDABLE_TABS_BORDER)
    }

    /// The open box: the stacked sizer's natural box plus the shell border,
    /// floored at the closed one (`openSize`).
    fn open_box(&self) -> Size {
        let closed = self.closed_size;
        let mut content = Size::ZERO;
        for size in &self.panel_sizes {
            content.width = content.width.max(size.width);
            content.height = content.height.max(size.height);
        }
        if content.width <= 0.0 && content.height <= 0.0 {
            return closed;
        }
        let natural = Size::new(
            content.width + EXPANDABLE_TABS_PANEL_PADDING * 2.0,
            content.height
                + EXPANDABLE_TABS_PANEL_PADDING
                + EXPANDABLE_TABS_BAR_HEIGHT
                + EXPANDABLE_TABS_PANEL_DOCK_GAP,
        );
        Size::new(
            (natural.width + EXPANDABLE_TABS_BORDER).max(closed.width),
            (natural.height + EXPANDABLE_TABS_BORDER).max(closed.height),
        )
    }

    /// The height of the band the panel is clipped to — everything above the
    /// docked bar and its clearance, inside the shell's own border.
    fn panel_band(&self) -> f64 {
        (self.shell_h.value()
            - EXPANDABLE_TABS_BORDER / 2.0
            - EXPANDABLE_TABS_BAR_HEIGHT
            - EXPANDABLE_TABS_PANEL_DOCK_GAP)
            .max(0.0)
    }

    /// Lay the bar out across the shell's current width, honouring upstream's
    /// `justify-between` (extra room spreads the tabs; `gap-1` is the floor).
    ///
    /// The bar spans the **content** box, so the shell's own border is off the
    /// available width before `p-2` is: `ROOT_BORDER` is a border on each side,
    /// not part of the row the tabs are packed into.
    fn place_tabs(&mut self) {
        let count = self.entries.len();
        if count == 0 {
            return;
        }
        let pad = (EXPANDABLE_TABS_BORDER + EXPANDABLE_TABS_BAR_INSET) / 2.0;
        let inner =
            (self.shell_w.value() - EXPANDABLE_TABS_BORDER - EXPANDABLE_TABS_BAR_INSET).max(0.0);
        let occupied: f64 = self.entries.iter().map(|e| e.width.value().max(0.0)).sum();
        let gap = if count > 1 {
            ((inner - occupied) / (count - 1) as f64).max(EXPANDABLE_TABS_BAR_GAP)
        } else {
            0.0
        };
        let mut x = pad;
        for entry in &mut self.entries {
            entry.x = x;
            x += entry.width.value().max(0.0) + gap;
        }
    }

    /// The tab under a widget-local `pos`, if any.
    fn hit_tab(&self, pos: Point) -> Option<usize> {
        (0..self.entries.len()).find(|index| self.entries[*index].rect(self.bar_top).contains(pos))
    }

    /// Report the value pressing tab `index` requests: `None` when it is
    /// already open (`setActive(isActive ? null : item.id)`).
    fn request_tab(&mut self, ctx: &mut EventCtx, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        let next = if self.active() == Some(index) {
            None
        } else {
            Some(entry.id.clone())
        };
        (self.on_value_change)(ctx, next);
    }

    /// Advance every lane and presence to `now`, returning whether anything is
    /// still moving.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.shell_w.snap();
            self.shell_h.snap();
            for entry in &mut self.entries {
                entry.width.snap();
                entry.reveal.snap();
            }
            for presence in &mut self.presences {
                *presence = presence.collapsed();
            }
        }
        let mut moving = self.shell_w.advance(now);
        moving |= self.shell_h.advance(now);
        for entry in &mut self.entries {
            moving |= entry.width.advance(now);
            moving |= entry.reveal.advance(now);
        }
        for presence in &mut self.presences {
            presence.advance(now);
            moving |= presence.is_animating();
        }
        moving
    }

    /// The stage panel `index` is drawn at: `(alpha, lift, scale)`.
    ///
    /// The entrance and the exit are staged from different lifts
    /// (`enter: y -8`, `exit: y -6`), so the phase picks which.
    fn panel_stage(&self, index: usize, now: FrameTime) -> (f64, f64, f64) {
        let Some(presence) = self.presences.get(index) else {
            return (0.0, 0.0, 1.0);
        };
        let p = presence.presence(now).clamp(0.0, 1.0);
        let lift = match presence.phase() {
            PresencePhase::Exiting => EXPANDABLE_TABS_CONTENT_EXIT_LIFT,
            _ => EXPANDABLE_TABS_CONTENT_ENTER_LIFT,
        };
        (
            p,
            -lift * (1.0 - p),
            EXPANDABLE_TABS_CONTENT_SCALE + (1.0 - EXPANDABLE_TABS_CONTENT_SCALE) * p,
        )
    }

    /// Paint one tab: its active pill, then its clipped label reveal.
    fn paint_tab(
        &self,
        origin: Point,
        scene: &mut dyn PaintScene,
        colors: &ExpandableTabsColors,
        index: usize,
    ) {
        let entry = &self.entries[index];
        let rect = entry.rect(self.bar_top);
        let at = origin + rect.origin().to_vec2();
        let is_active = self.active() == Some(index);

        if is_active {
            scene.fill_rounded_rect(
                at,
                rect.size(),
                style::resolve_radius(EXPANDABLE_TABS_TAB_RADIUS, rect.width(), rect.height()),
                style::with_alpha(colors.ink, EXPANDABLE_TABS_PILL_ALPHA),
            );
        }

        let reveal = entry.reveal.value().clamp(0.0, 1.0);
        let label_width = entry.label_width * reveal;
        if label_width <= 0.0 {
            return;
        }
        // The label rides `marginLeft: 0 → LABEL_GAP` out of the icon's right
        // edge, clipped to its own animated width (`overflow-hidden`).
        let label_x = rect.x0
            + entry.icon_left()
            + EXPANDABLE_TABS_ICON_BOX
            + EXPANDABLE_TABS_LABEL_GAP * reveal;
        let text = entry.run.size();
        let clip_origin = origin + Point::new(label_x, rect.y0).to_vec2();
        scene.push_clip(clip_origin, Size::new(label_width, rect.height()));
        let ink = if is_active {
            colors.ink
        } else {
            colors.dim_ink
        };
        entry.run.paint(
            Point::new(
                clip_origin.x,
                clip_origin.y + (rect.height() - text.height) / 2.0,
            ),
            style::with_alpha(ink, reveal as f32),
            scene,
        );
        scene.pop_clip();
    }

    /// The keyboard arm: arrows move the roving highlight, `Space`/`Enter`
    /// activates it, `Escape` closes the shell.
    fn handle_key(&mut self, ctx: &mut EventCtx, key: &KeyEvent) -> EventResult {
        if matches!(key.key, Key::Named(NamedKey::Escape)) {
            if self.value.is_none() {
                return EventResult::Ignored;
            }
            (self.on_value_change)(ctx, None);
            return EventResult::Handled;
        }
        if let Some(step) = arrow_step(key) {
            let len = self.entries.len();
            if len == 0 {
                return EventResult::Ignored;
            }
            self.focused = (self.focused as isize + step).rem_euclid(len as isize) as usize;
            ctx.request_redraw();
            return EventResult::Handled;
        }
        if is_activation_key(key) && self.focused < self.entries.len() {
            let index = self.focused;
            self.request_tab(ctx, index);
            return EventResult::Handled;
        }
        EventResult::Ignored
    }
}

impl Widget for ExpandableTabsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let text_style = label_style(Theme::from_layout_ctx(ctx));
        for entry in &mut self.entries {
            entry.label_width = entry.run.layout(ctx, &text_style).width;
        }
        self.closed_size = self.closed_box();

        // **Every** panel is measured, open or not — upstream's hidden sizer
        // renders all of them into one grid cell on every commit, and that is
        // exactly what makes the open box stable across a tab switch. Only a
        // visible panel is painted, routed to or published.
        let panel_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(
                (bc.max().width - EXPANDABLE_TABS_PANEL_PADDING * 2.0 - EXPANDABLE_TABS_BORDER)
                    .max(0.0),
                (bc.max().height
                    - EXPANDABLE_TABS_PANEL_PADDING
                    - EXPANDABLE_TABS_BAR_HEIGHT
                    - EXPANDABLE_TABS_PANEL_DOCK_GAP
                    - EXPANDABLE_TABS_BORDER)
                    .max(0.0),
            ),
        );
        let panel_inset = EXPANDABLE_TABS_BORDER / 2.0 + EXPANDABLE_TABS_PANEL_PADDING;
        for index in 0..self.panels.len() {
            let Some(pod) = self.panels.get_mut(index) else {
                continue;
            };
            let size = pod.layout_child(ctx, &panel_bc);
            pod.set_origin(Point::new(panel_inset, panel_inset));
            self.panel_sizes[index] = size;
        }
        self.open_size = self.open_box();

        // Retarget the shell, the tab widths and the label reveals — `layout`
        // is the pass that knows the numbers, `paint` is the pass with a clock
        // (see the module docs).
        let active = self.active();
        let target = match active {
            Some(_) => self.open_size,
            None => self.closed_size,
        };
        if self.sized {
            self.shell_w.retarget(target.width);
            self.shell_h.retarget(target.height);
        } else {
            self.shell_w = Lane::at_rest(Ramp::spring(EXPANDABLE_TABS_SHELL_SPRING), target.width);
            self.shell_h = Lane::at_rest(Ramp::spring(EXPANDABLE_TABS_SHELL_SPRING), target.height);
            self.sized = true;
        }
        for index in 0..self.entries.len() {
            let is_active = active == Some(index);
            let width = if is_active {
                self.active_width(index)
            } else {
                EXPANDABLE_TABS_TAB_WIDTH
            };
            self.entries[index].width.retarget(width);
            // The reveal's ramp is asymmetric upstream: a spring in, a short
            // tween out.
            let (ramp, to) = if is_active {
                (Ramp::spring(EXPANDABLE_TABS_LABEL_OPEN), 1.0)
            } else {
                (EXPANDABLE_TABS_LABEL_CLOSE, 0.0)
            };
            self.entries[index].reveal.retarget_with(ramp, to);
        }

        let shell = Size::new(self.shell_w.value().max(0.0), self.shell_h.value().max(0.0));
        self.bar_top = (shell.height - EXPANDABLE_TABS_BORDER / 2.0 - EXPANDABLE_TABS_BAR_HEIGHT
            + (EXPANDABLE_TABS_BAR_HEIGHT - EXPANDABLE_TABS_TAB_HEIGHT) / 2.0)
            .max(0.0);
        self.place_tabs();

        let icon_bc = BoxConstraints::tight(Size::new(
            EXPANDABLE_TABS_ICON_BOX,
            EXPANDABLE_TABS_ICON_BOX,
        ));
        let bar_top = self.bar_top;
        for index in 0..self.entries.len() {
            let x = self.entries[index].x + self.entries[index].icon_left();
            let y = bar_top + (EXPANDABLE_TABS_TAB_HEIGHT - EXPANDABLE_TABS_ICON_BOX) / 2.0;
            if let Some(pod) = self.icons.get_mut(index) {
                pod.layout_child(ctx, &icon_bc);
                pod.set_origin(Point::new(x, y));
            }
        }
        bc.constrain(shell)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The authoritative hover read: no link on this widget's path at all
        // means the latch is stale.
        if !ctx.is_hovered() {
            self.hovered = None;
        }
        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let moving = self.advance(now, reduce_motion);
        let origin = ctx.origin();
        let shell = ctx.size();

        let radius = style::resolve_radius(EXPANDABLE_TABS_RADIUS, shell.width, shell.height);
        scene.fill_rounded_rect(origin, shell, radius, colors.surface);
        crate::press::stroke_outline(scene, origin, shell, radius, colors.border);

        // The panels, clipped to the band above the docked bar
        // (`overflow-hidden` with a `bottom: BAR_H + PANEL_DOCK_GAP` reserve).
        let band = self.panel_band();
        if band > 0.0 {
            scene.push_clip(
                Point::new(origin.x, origin.y + EXPANDABLE_TABS_BORDER / 2.0),
                Size::new(shell.width - EXPANDABLE_TABS_BORDER, band),
            );
            for index in 0..self.panels.len() {
                if !self.panel_visible(index) {
                    continue;
                }
                let (alpha, lift, scale) = self.panel_stage(index, now);
                let Some(pod) = self.panels.get_mut(index) else {
                    continue;
                };
                // `transformOrigin: top center` — the panel unfurls downward
                // out of the shell's top edge.
                let pivot = Point::new(origin.x + shell.width / 2.0, origin.y);
                scene.push_layer(origin, Size::new(shell.width, band), alpha as f32);
                scene.push_transform(
                    Affine::translate((0.0, lift))
                        * Affine::translate(pivot.to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-pivot.to_vec2()),
                );
                pod.paint_child(ctx, scene);
                scene.pop_transform();
                scene.pop_layer();
            }
            scene.pop_clip();
        }

        // The bar, over the panel band.
        for index in 0..self.entries.len() {
            self.paint_tab(origin, scene, &colors, index);
        }
        for pod in &mut self.icons {
            pod.paint_child(ctx, scene);
        }

        // Three of the four animated quantities resize the widget, so a moving
        // shell asks for a relayout rather than a bare repaint.
        if moving {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in self.icons.iter_mut().chain(self.panels.iter_mut()) {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The open panel is routed FIRST, so the bar's own hover claim below is
        // a fallback rather than a pre-emption.
        if let Some(index) = self.active()
            && let Some(pod) = self.panels.get_mut(index)
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }

        match event {
            InputEvent::Key(key) => self.handle_key(ctx, key),
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_tab(p.position) else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(index);
                    self.focused = index;
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
                    let over = self.hit_tab(p.position);
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
                    if self.hit_tab(p.position) == Some(armed) {
                        self.request_tab(ctx, armed);
                    }
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    if self.captured.take().is_none() {
                        return EventResult::Ignored;
                    }
                    EventResult::Handled
                }
            },
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let active = self.active();
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for (index, entry) in self.entries.iter().enumerate() {
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(entry.label.as_str());
                        node.set_selected(active == Some(index));
                        node.add_action(Action::Click);
                    });
                }
            },
        );
        // Only the panel input can reach is published — the input-parity
        // carve-out for a container that gates input.
        if let Some(pod) = active.and_then(|index| self.panels.get(index)) {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(icons, panels);
}

/// Which way an arrow key moves along the bar, or `None`.
fn arrow_step(key: &KeyEvent) -> Option<isize> {
    match &key.key {
        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => Some(1),
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => Some(-1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, Brush, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::text;
    use std::any::Any;

    /// Records what the widget paints.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn push_transform(&mut self, _t: Affine) {}
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
    }

    #[derive(Default)]
    struct Picked {
        last: Option<Option<String>>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<ExpandableTabsItem<Picked>> {
        vec![
            expandable_tabs_item("home", "Home", text("H"), text("the home panel")),
            expandable_tabs_item(
                "activity",
                "Activity and more",
                text("A"),
                text("the activity panel, which is wider"),
            ),
            expandable_tabs_item("settings", "Settings", text("S"), text("settings")),
        ]
    }

    fn view(value: Option<&str>) -> ExpandableTabsView<Picked> {
        expandable_tabs::<Picked, _>(
            value.map(str::to_string),
            items(),
            |s: &mut Picked, v: Option<String>| {
                s.last = Some(v);
                s.count += 1;
            },
        )
    }

    fn build(value: Option<&str>) -> ExpandableTabsWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(value), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut ExpandableTabsWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, 400.0)),
        )
    }

    fn laid_out(value: Option<&str>) -> (ExpandableTabsWidget, Size) {
        let mut w = build(value);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(w: &mut ExpandableTabsWidget, size: Size, ms: f64) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    /// Re-run the view with a new value, exactly as a rebuild would.
    fn retarget(w: &mut ExpandableTabsWidget, from: Option<&str>, to: Option<&str>) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(to), &view(from), w, &mut ctx);
    }

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn key_event(named: NamedKey) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(named),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(w: &mut ExpandableTabsWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// Settle a widget by painting far past every ramp, re-laying out between
    /// passes so the layout animation actually lands.
    fn settle(w: &mut ExpandableTabsWidget, size: Size, from_ms: f64) -> Size {
        let mut size = size;
        for step in 0..40 {
            paint_at(w, size, from_ms + step as f64 * 400.0);
            size = layout(w);
        }
        size
    }

    /// The closed shell is exactly upstream's `closedSize` — the packed bar and
    /// nothing else — and it owes no frame.
    #[test]
    fn the_closed_shell_is_the_packed_bar() {
        let (mut w, size) = laid_out(None);
        let expected = w.closed_box();
        assert_eq!(size, expected);
        assert_eq!(
            expected.width,
            3.0 * EXPANDABLE_TABS_TAB_WIDTH
                + 2.0 * EXPANDABLE_TABS_BAR_GAP
                + EXPANDABLE_TABS_BAR_INSET
                + EXPANDABLE_TABS_BORDER
        );
        assert_eq!(
            expected.height,
            EXPANDABLE_TABS_BAR_HEIGHT + EXPANDABLE_TABS_BORDER
        );
        let (_, needs_frame) = paint_at(&mut w, size, 0.0);
        assert!(!needs_frame, "a settled closed shell animates nothing");
    }

    /// The bar packs its tabs at the `gap-1` floor while closed, in declaration
    /// order, with the row centred in the 52px dock.
    #[test]
    fn the_bar_packs_its_tabs_at_the_gap_floor_when_closed() {
        let (w, _) = laid_out(None);
        let first = w.entries[0].rect(w.bar_top);
        let second = w.entries[1].rect(w.bar_top);
        let third = w.entries[2].rect(w.bar_top);
        assert_eq!(
            first.x0,
            (EXPANDABLE_TABS_BORDER + EXPANDABLE_TABS_BAR_INSET) / 2.0
        );
        assert_eq!(first.width(), EXPANDABLE_TABS_TAB_WIDTH);
        assert_eq!(second.x0, first.x1 + EXPANDABLE_TABS_BAR_GAP);
        assert_eq!(third.x0, second.x1 + EXPANDABLE_TABS_BAR_GAP);
        assert_eq!(
            w.bar_top,
            w.shell_h.value() - EXPANDABLE_TABS_BORDER / 2.0 - EXPANDABLE_TABS_BAR_HEIGHT
                + (EXPANDABLE_TABS_BAR_HEIGHT - EXPANDABLE_TABS_TAB_HEIGHT) / 2.0
        );
    }

    /// Opening springs the shell from the closed box to the open one: strictly
    /// between them mid-flight, and exactly on the open box once settled. The
    /// core retarget property.
    #[test]
    fn opening_springs_the_shell_from_the_closed_box_to_the_open_one() {
        let (mut w, size) = laid_out(None);
        let closed = w.closed_size;
        paint_at(&mut w, size, 0.0);

        retarget(&mut w, None, Some("home"));
        let size = layout(&mut w);
        assert_eq!(size, closed, "the first frame is still the closed box");

        let (_, needs_frame) = paint_at(&mut w, size, 100.0);
        assert!(needs_frame, "a growing shell owes frames");
        let size = layout(&mut w);
        paint_at(&mut w, size, 260.0);
        let mid = layout(&mut w);
        assert!(
            mid.height > closed.height && mid.height < w.open_size.height,
            "mid-flight: {mid:?} between {closed:?} and {:?}",
            w.open_size
        );

        let settled = settle(&mut w, mid, 300.0);
        assert!(
            (settled.height - w.open_size.height).abs() < 0.01,
            "settles on the open box: {settled:?} vs {:?}",
            w.open_size
        );
        assert!(settled.height > closed.height);
    }

    /// The open box is the max over *every* panel, so switching tabs moves no
    /// shell edge — upstream's stacked sizer grid, ported.
    #[test]
    fn the_open_box_is_the_same_whichever_tab_is_open() {
        let (mut home, size) = laid_out(Some("home"));
        let home_size = settle(&mut home, size, 0.0);
        let (mut activity, size) = laid_out(Some("activity"));
        let activity_size = settle(&mut activity, size, 0.0);
        assert!(
            (home_size.width - activity_size.width).abs() < 0.01
                && (home_size.height - activity_size.height).abs() < 0.01,
            "{home_size:?} vs {activity_size:?}"
        );
    }

    /// The active tab widens to hold its label and the others stay square; the
    /// reveal runs with it and lands at exactly 1.
    #[test]
    fn the_active_tab_expands_to_its_labelled_width() {
        let (mut w, size) = laid_out(Some("activity"));
        settle(&mut w, size, 0.0);
        let expected = w.active_width(1);
        assert!(
            expected > EXPANDABLE_TABS_TAB_WIDTH,
            "a labelled tab is wider than the collapsed box"
        );
        assert!((w.entries[1].width.value() - expected).abs() < 0.01);
        assert_eq!(w.entries[0].width.value(), EXPANDABLE_TABS_TAB_WIDTH);
        assert_eq!(w.entries[2].width.value(), EXPANDABLE_TABS_TAB_WIDTH);
        assert!((w.entries[1].reveal.value() - 1.0).abs() < 1e-9);
        assert_eq!(w.entries[0].reveal.value(), 0.0);
    }

    /// The labelled width is upstream's own formula, floored at the collapsed
    /// box, and the icon slides from centred to `pl-2.5` with the reveal.
    #[test]
    fn the_labelled_width_is_the_upstream_formula() {
        let (mut w, _) = laid_out(Some("home"));
        layout(&mut w);
        let label = w.entries[0].label_width;
        assert!(label > 0.0, "the label was measured");
        assert_eq!(
            w.active_width(0),
            EXPANDABLE_TABS_TAB_WIDTH.max(
                EXPANDABLE_TABS_ACTIVE_PAD_LEFT
                    + EXPANDABLE_TABS_ICON_BOX
                    + EXPANDABLE_TABS_LABEL_GAP
                    + label
                    + EXPANDABLE_TABS_ACTIVE_PAD_RIGHT
            )
        );
        assert_eq!(
            w.entries[2].icon_left(),
            (EXPANDABLE_TABS_TAB_WIDTH - EXPANDABLE_TABS_ICON_BOX) / 2.0,
            "a collapsed tab centres its icon"
        );
    }

    /// The reveal is asymmetric, as upstream authors it: the entrance is the
    /// 380ms spring, the exit the 160ms tween, so a close finishes sooner than
    /// an open of the same distance.
    #[test]
    fn the_label_reveal_opens_on_a_spring_and_closes_on_a_shorter_tween() {
        let (mut open, size) = laid_out(None);
        paint_at(&mut open, size, 0.0);
        retarget(&mut open, None, Some("home"));
        layout(&mut open);
        paint_at(&mut open, size, 200.0);
        layout(&mut open);
        paint_at(&mut open, size, 361.0);
        assert!(
            open.entries[0].reveal.value() < 1.0,
            "the 380ms entrance is still running at 361ms"
        );

        let (mut close, size) = laid_out(Some("home"));
        let size = settle(&mut close, size, 0.0);
        retarget(&mut close, Some("home"), None);
        layout(&mut close);
        paint_at(&mut close, size, 20_000.0);
        layout(&mut close);
        paint_at(&mut close, size, 20_161.0);
        assert_eq!(
            close.entries[0].reveal.value(),
            0.0,
            "the 160ms exit has landed"
        );
    }

    /// The outgoing panel stays laid out through its exit and the incoming one
    /// enters at the same time — `AnimatePresence mode="popLayout"`, ported.
    #[test]
    fn a_switch_keeps_the_outgoing_panel_alive_through_its_exit() {
        let (mut w, size) = laid_out(Some("home"));
        let size = settle(&mut w, size, 0.0);
        assert!(w.panel_visible(0) && !w.panel_visible(1));

        retarget(&mut w, Some("home"), Some("activity"));
        layout(&mut w);
        paint_at(&mut w, size, 20_020.0);
        assert!(
            w.panel_visible(0) && w.panel_visible(1),
            "both panels are alive mid-swap"
        );
        assert_eq!(w.presences[0].phase(), PresencePhase::Exiting);
        assert_eq!(w.presences[1].phase(), PresencePhase::Entering);

        settle(&mut w, size, 20_100.0);
        assert!(!w.panel_visible(0), "the exit has finished");
        assert!(w.panel_visible(1));
    }

    /// An entering panel is staged from above at reduced alpha and scale, and
    /// lands square — the `enter → center` variant, minus the blur.
    #[test]
    fn an_entering_panel_is_lifted_faded_and_scaled() {
        let (mut w, size) = laid_out(None);
        paint_at(&mut w, size, 0.0);
        retarget(&mut w, None, Some("home"));
        let mut size = layout(&mut w);
        // The panel band only opens once the springing shell is taller than the
        // dock reserve, exactly as upstream's `bottom: 56px` container does.
        let mut rec = Recorder::default();
        for step in 0..8 {
            let (frame, _) = paint_at(&mut w, size, 100.0 + step as f64 * 30.0);
            rec = frame;
            size = layout(&mut w);
        }
        assert!(
            rec.layers.iter().any(|a| *a < 1.0),
            "the entering panel is faded: {:?}",
            rec.layers
        );
        let (alpha, lift, scale) = w.panel_stage(0, ft_ms(310.0));
        assert!((0.0..1.0).contains(&alpha));
        assert!((-EXPANDABLE_TABS_CONTENT_ENTER_LIFT..0.0).contains(&lift));
        assert!((EXPANDABLE_TABS_CONTENT_SCALE..1.0).contains(&scale));

        let size = settle(&mut w, size, 400.0);
        let (alpha, lift, scale) = w.panel_stage(0, ft_ms(30_000.0));
        assert_eq!((alpha, lift, scale), (1.0, 0.0, 1.0));
        assert!(size.height > w.closed_size.height);
    }

    /// A leaving panel is staged from the *exit* lift, not the entrance one —
    /// the two constants upstream authors separately.
    #[test]
    fn a_leaving_panel_uses_the_exit_lift() {
        let (mut w, size) = laid_out(Some("home"));
        let size = settle(&mut w, size, 0.0);
        retarget(&mut w, Some("home"), None);
        layout(&mut w);
        // The first paint latches the exit's clock; the second is far enough in
        // to read a partly-faded panel.
        paint_at(&mut w, size, 20_000.0);
        layout(&mut w);
        paint_at(&mut w, size, 20_040.0);
        assert_eq!(w.presences[0].phase(), PresencePhase::Exiting);
        let (alpha, lift, _) = w.panel_stage(0, ft_ms(20_040.0));
        assert!(alpha < 1.0);
        assert!(
            (-EXPANDABLE_TABS_CONTENT_EXIT_LIFT..=0.0).contains(&lift),
            "the exit lift bounds the travel: {lift}"
        );
    }

    /// Pressing a closed tab requests it; pressing the open one requests `None`
    /// — upstream's `setActive(isActive ? null : item.id)`. The widget never
    /// writes its own value.
    #[test]
    fn pressing_a_tab_reports_it_and_pressing_the_open_one_closes() {
        let (mut w, size) = laid_out(None);
        let mut state = Picked::default();
        let hit = w.entries[1].rect(w.bar_top).center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, hit), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, hit), &mut state);
        assert_eq!(state.last, Some(Some("activity".to_string())));
        assert_eq!(state.count, 1);
        assert_eq!(w.value, None, "the widget never writes its own value");

        let (mut open, size) = laid_out(Some("activity"));
        let size = settle(&mut open, size, 0.0);
        let hit = open.entries[1].rect(open.bar_top).center();
        dispatch(
            &mut open,
            size,
            &pointer(PointerPhase::Down, hit),
            &mut state,
        );
        dispatch(&mut open, size, &pointer(PointerPhase::Up, hit), &mut state);
        assert_eq!(state.last, Some(None), "the open tab closes the shell");
    }

    /// A release that misses the armed tab reports nothing, and a cancelled
    /// press is dropped.
    #[test]
    fn a_release_off_the_armed_tab_reports_nothing() {
        let (mut w, size) = laid_out(None);
        let mut state = Picked::default();
        let first = w.entries[0].rect(w.bar_top).center();
        let second = w.entries[1].rect(w.bar_top).center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, first),
            &mut state,
        );
        dispatch(&mut w, size, &pointer(PointerPhase::Up, second), &mut state);
        assert_eq!(state.count, 0);

        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, first),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Cancel, first),
            &mut state,
        );
        dispatch(&mut w, size, &pointer(PointerPhase::Up, first), &mut state);
        assert_eq!(state.count, 0, "a cancelled press never activates");
    }

    /// Escape closes an open shell and is ignored by a closed one; the arrows
    /// move the roving highlight and activation reports it.
    #[test]
    fn escape_closes_and_the_arrows_move_the_highlight() {
        let (mut w, size) = laid_out(Some("home"));
        let mut state = Picked::default();
        dispatch(&mut w, size, &key_event(NamedKey::Escape), &mut state);
        assert_eq!(state.last, Some(None));

        let (mut closed, size) = laid_out(None);
        let mut state = Picked::default();
        dispatch(&mut closed, size, &key_event(NamedKey::Escape), &mut state);
        assert_eq!(state.count, 0, "a closed shell has nothing to close");

        dispatch(
            &mut closed,
            size,
            &key_event(NamedKey::ArrowRight),
            &mut state,
        );
        assert_eq!(closed.focused, 1);
        dispatch(
            &mut closed,
            size,
            &key_event(NamedKey::ArrowLeft),
            &mut state,
        );
        dispatch(
            &mut closed,
            size,
            &key_event(NamedKey::ArrowLeft),
            &mut state,
        );
        assert_eq!(closed.focused, 2, "the highlight wraps");
        dispatch(&mut closed, size, &key_event(NamedKey::Enter), &mut state);
        assert_eq!(state.last, Some(Some("settings".to_string())));
    }

    /// `reduce_motion` collapses every ramp: the shell is on its target box on
    /// the first frame and nothing owes a frame.
    #[test]
    fn reduced_motion_lands_the_shell_at_once() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(None);
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(0.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);

        retarget(&mut w, None, Some("home"));
        let size = layout(&mut w);
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(16.0)).with_theme(&theme);
        w.paint(&mut ctx, &mut rec);
        assert!(!ctx.needs_frame(), "a collapsed ramp owes no frame");
        let landed = layout(&mut w);
        assert_eq!(landed, w.open_size);
        assert_eq!(w.entries[0].reveal.value(), 1.0);
    }

    /// The shell paints its own card surface, and the open tab paints a
    /// `foreground/10` pill under its clipped, revealed label.
    #[test]
    fn the_shell_and_the_active_pill_paint_their_tokens() {
        let (mut w, size) = laid_out(Some("home"));
        let size = settle(&mut w, size, 0.0);
        let (rec, _) = paint_at(&mut w, size, 30_000.0);
        assert!(
            rec.rrects
                .iter()
                .any(|(_, _, _, c)| *c == crate::BEUI_LIGHT.card),
            "the shell fills with `card`"
        );
        let pill = style::with_alpha(crate::BEUI_LIGHT.foreground, EXPANDABLE_TABS_PILL_ALPHA);
        assert!(
            rec.rrects.iter().any(|(_, _, _, c)| *c == pill),
            "the open tab paints its pill: {:?}",
            rec.rrects
        );
        assert!(
            rec.clips.len() >= 2,
            "the panel band and the revealed label are both clipped"
        );
        assert!(!rec.inks.is_empty(), "the revealed label is painted");
    }

    /// The converted springs settle at exactly the durations upstream states —
    /// the transcription check for the `{ duration, bounce }` conversion.
    #[test]
    fn the_converted_springs_settle_at_the_upstream_durations() {
        let pairs = [
            (EXPANDABLE_TABS_SHELL_SPRING, 580.0),
            (EXPANDABLE_TABS_TAB_SPRING, 460.0),
            (EXPANDABLE_TABS_LABEL_OPEN, 380.0),
            (EXPANDABLE_TABS_CONTENT_SPRING, 460.0),
        ];
        for (spring, expected_ms) in pairs {
            let settle = Ramp::spring(spring).settle().as_secs_f64() * 1000.0;
            assert!(
                (settle - expected_ms).abs() < 5.0,
                "{spring:?} settles at {settle}ms, expected {expected_ms}ms"
            );
        }
        assert_eq!(
            EXPANDABLE_TABS_LABEL_CLOSE.settle(),
            Duration::from_millis(160)
        );
        assert_eq!(
            EXPANDABLE_TABS_CONTENT_EXIT.settle(),
            Duration::from_millis(80)
        );
    }

    /// The block publishes a tab list with one selectable tab per item.
    #[test]
    fn semantics_publish_one_tab_node_per_item() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = Picked::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut Picked| view(Some("activity"));
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(600.0, 400.0), &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3, "one node per item");
        assert_eq!(
            tabs.iter()
                .filter(|(_, n)| n.is_selected() == Some(true))
                .count(),
            1,
            "exactly one selected tab"
        );
    }

    // ---- Typeface: the tab labels follow the live theme ---------------------

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(600.0, 400.0);

    /// Two tabs with glyph-free icons and panels, the first open so its label
    /// is on screen.
    fn probe_view(_: &mut ()) -> ExpandableTabsView<()> {
        let blank = || frust::SizedBox::<()>(Some(16.0), Some(16.0));
        expandable_tabs::<(), _>(
            Some("home".to_string()),
            vec![
                expandable_tabs_item("home", "Home", blank(), blank()),
                expandable_tabs_item("settings", "Settings", blank(), blank()),
            ],
            |_: &mut (), _| {},
        )
    }

    #[test]
    fn the_tab_labels_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the tab labels", probe_view, PROBE_WINDOW);
        let runs = Probe::new(probe_view, PROBE_WINDOW, crate::theme()).frame();
        assert_eq!(runs.len(), 1, "the open tab's label");
    }

    #[test]
    fn the_tab_labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the tab labels", probe_view, PROBE_WINDOW);
    }
}
