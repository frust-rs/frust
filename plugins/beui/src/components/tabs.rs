//! Ports beUI's `tabs` component.
//!
//! **Source:** `components/motion/tabs.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! Four upstream parts (`Tabs`, `TabsList`, `TabsTrigger`, `TabsContent`)
//! collapse into one widget: the strip, its triggers and the visible panel are
//! one interactive unit — a trigger's metrics depend on the list's variant, the
//! indicator's travel spans the whole strip, and arrow-key traversal is a
//! property of the set rather than of a trigger.
//!
//! | upstream | here |
//! |---|---|
//! | `variant="pill"` list `gap-1 rounded-full bg-card p-1` | [`TabsVariant::Pill`], [`TABS_LIST_GAP`], [`TABS_PILL_PADDING`] |
//! | `variant="segment"` list `gap-0 rounded-lg bg-card p-0.5` | [`TabsVariant::Segment`], [`TABS_SEGMENT_PADDING`] |
//! | `variant="underline"` list `gap-1 border-b border-border` | [`TabsVariant::Underline`] over a hairline rule |
//! | trigger `px-3.5 py-1.5 text-sm font-medium` | [`TABS_TRIGGER_PADDING_X`]/[`TABS_TRIGGER_PADDING_Y`], [`style::TEXT_SM`] |
//! | underline trigger `px-3 min-h-[44px]` | [`TABS_UNDERLINE_PADDING_X`], [`TABS_UNDERLINE_MIN_HEIGHT`] |
//! | `<motion.span layoutId>` `absolute inset-0 bg-primary` | the sprung indicator pill |
//! | underline `absolute -bottom-px h-px bg-primary` | the sprung indicator bar, [`TABS_INDICATOR_THICKNESS`] |
//! | active `text-primary-foreground` / `text-foreground` | the resolved active ink, per variant |
//! | inactive `text-muted-foreground hover:text-foreground` | the dimmed ink, brightening under the pointer |
//! | `TabsContent` `mt-4`, `{opacity:0,y:4} → {opacity:1,y:0}` | [`TABS_CONTENT_GAP`], [`TABS_CONTENT_RISE`], [`TABS_CONTENT_TIMING`] |
//!
//! # The indicator is a *rect* spring, not a cross-fade
//!
//! Upstream's indicator is a Motion `layoutId` element: React unmounts it from
//! the losing trigger and mounts it on the winning one, and the projection
//! engine animates the box between the two measured rects. This port keeps that
//! shape — the widget remembers the rect the indicator was displaying when the
//! value changed and springs the whole rect (x and width both) onto the new
//! trigger's, so a wide tab stretches into a narrow one exactly as the web
//! original does.
//!
//! The spring is [`TABS_INDICATOR_SPRING`], upstream's own local `transition`
//! const — **not** [`SPRING_LAYOUT`](crate::tokens::motion::SPRING_LAYOUT).
//! `tabs.tsx` authors a weightier spring inline (`stiffness: 170, damping: 24,
//! mass: 1.2`, commented "a touch of overshoot so it settles with life instead
//! of snapping"), and substituting the shared token would make the travel
//! visibly quicker and remove the overshoot entirely, since every spring in
//! [`crate::tokens::motion`] is authored near critical damping. The number that
//! carries this component's character wins over the number that is shared.
//!
//! Travel is driven from **raw** spring progress, so the indicator passes its
//! target rect and settles back. Under `motion.reduce_motion` it snaps and owes
//! no frame, which is upstream's `{ duration: 0 }` `MotionConfig` arm.
//!
//! # Controlled, with automatic activation
//!
//! `value` is a prop. A press, an arrow key and `Space`/`Enter` all *report* the
//! requested value through `on_value_change` and never write it locally, so the
//! app stays the single source of truth. Arrow keys move the roving focus **and**
//! activate, matching upstream's click-to-`setValue` and the automatic-activation
//! convention the sibling catalogs keep.
//!
//! # Degradations against the web original
//!
//! - **Hidden panels are built but not laid out.** Upstream renders every
//!   inactive panel into the DOM behind `hidden` so a crawler and a screen
//!   reader still see it. Here every panel is a `ChildPod` from the first build,
//!   but only the active one is laid out, painted, routed to and published to
//!   semantics — the input-parity carve-out a container that gates input is
//!   allowed. frust has no "mounted but inert" state, and publishing an
//!   unmeasured panel would give it a stale or zero box.
//! - **No `layoutRoot` scoping.** Upstream scopes projection to the wrapper
//!   because `layoutId` measures in page coordinates. The indicator here is
//!   computed from widget-local trigger rects, so there is no scroll offset to
//!   replay and nothing to scope.
//! - **No exit for the outgoing panel.** Upstream has none either (`TabsContent`
//!   is `initial`/`animate` with no `exit`), so the swap is a hard cut into the
//!   incoming panel's entrance.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx,
    EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any, build_child,
    erase_callback_arg, rebuild_children, route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::motion::Ramp;
use crate::press::{is_activation_key, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::EASE_OUT;

/// The gap between triggers, in logical px (`gap-1`; the segment variant packs
/// them flush at `gap-0`).
pub const TABS_LIST_GAP: f64 = 4.0;

/// The pill strip's inner padding, in logical px (`p-1`).
pub const TABS_PILL_PADDING: f64 = 4.0;

/// The segment strip's inner padding, in logical px (`p-0.5`).
pub const TABS_SEGMENT_PADDING: f64 = 2.0;

/// Horizontal padding per pill/segment trigger, in logical px (`px-3.5`).
pub const TABS_TRIGGER_PADDING_X: f64 = 14.0;

/// Vertical padding per pill/segment trigger, in logical px (`py-1.5`).
pub const TABS_TRIGGER_PADDING_Y: f64 = 6.0;

/// Horizontal padding per underline trigger, in logical px (`px-3`).
pub const TABS_UNDERLINE_PADDING_X: f64 = 12.0;

/// The underline trigger's minimum height, in logical px (`min-h-[44px]`) —
/// upstream's own tap-target floor, which is why it survives the port.
pub const TABS_UNDERLINE_MIN_HEIGHT: f64 = 44.0;

/// The underline indicator's thickness, in logical px (`h-px`).
pub const TABS_INDICATOR_THICKNESS: f64 = 1.0;

/// The gap between the strip and the panel, in logical px (`mt-4`).
pub const TABS_CONTENT_GAP: f64 = 16.0;

/// How far below its resting place an entering panel starts, in logical px
/// (`initial={{ y: 4 }}`).
pub const TABS_CONTENT_RISE: f64 = 4.0;

/// The panel entrance's shape: `{ duration: 0.18, ease: EASE_OUT }`.
pub const TABS_CONTENT_TIMING: Ramp = Ramp::eased(Duration::from_millis(180), EASE_OUT);

/// The indicator's travel spring — upstream's own local `transition` const, not
/// a shared token. See the [module docs](self) for why the local number wins.
pub const TABS_INDICATOR_SPRING: SpringDescription = SpringDescription {
    mass: 1.2,
    stiffness: 170.0,
    damping: 24.0,
};

/// The strip's `variant` axis (`listClasses`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabsVariant {
    /// `variant="pill"`: a `card` strip whose active tab is a full-radius
    /// `primary` pill.
    #[default]
    Pill,
    /// `variant="underline"`: a transparent strip over a hairline rule, with a
    /// `primary` bar under the active trigger.
    Underline,
    /// `variant="segment"`: a `card` strip whose active tab is a `rounded-lg`
    /// `primary` block, packed flush against its neighbours.
    Segment,
}

impl TabsVariant {
    /// The strip's inner padding.
    const fn padding(self) -> f64 {
        match self {
            TabsVariant::Pill => TABS_PILL_PADDING,
            TabsVariant::Segment => TABS_SEGMENT_PADDING,
            TabsVariant::Underline => 0.0,
        }
    }

    /// The gap between two triggers.
    const fn gap(self) -> f64 {
        match self {
            TabsVariant::Pill | TabsVariant::Underline => TABS_LIST_GAP,
            TabsVariant::Segment => 0.0,
        }
    }

    /// A trigger's horizontal padding.
    const fn trigger_padding_x(self) -> f64 {
        match self {
            TabsVariant::Pill | TabsVariant::Segment => TABS_TRIGGER_PADDING_X,
            TabsVariant::Underline => TABS_UNDERLINE_PADDING_X,
        }
    }

    /// The corner radius of this variant's strip and indicator.
    const fn radius(self) -> f64 {
        match self {
            TabsVariant::Segment => style::RADIUS_LG,
            _ => style::RADIUS_FULL,
        }
    }

    /// Whether the strip paints a filled background behind its triggers.
    const fn has_strip_fill(self) -> bool {
        matches!(self, TabsVariant::Pill | TabsVariant::Segment)
    }
}

/// One tab: the value it selects, its trigger label, and the panel shown while
/// it is active.
pub struct TabsTab<State: 'static> {
    value: String,
    label: String,
    disabled: bool,
    content: AnyView<State>,
}

/// Create a tab labelled `label` that selects `value` and shows `content` while
/// active.
pub fn tabs_tab<State: 'static, V: View<State>>(
    value: impl Into<String>,
    label: impl Into<String>,
    content: V,
) -> TabsTab<State> {
    TabsTab {
        value: value.into(),
        label: label.into(),
        disabled: false,
        content: any(content),
    }
}

impl<State: 'static> TabsTab<State> {
    /// Make this tab inert: dimmed, unpressable, skipped by arrow traversal.
    ///
    /// Upstream ships no disabled trigger; this is the catalog convention every
    /// other control here follows, and a tab set without it cannot express a
    /// gated panel at all.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this tab selects.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A view-held selection callback, erased on build.
type OnValueChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative beUI tab set. See the [module docs](self).
pub struct TabsView<State: 'static> {
    value: String,
    tabs: Vec<TabsTab<State>>,
    variant: TabsVariant,
    on_value_change: OnValueChange<State>,
}

/// Create a tab set whose active tab is the one whose value equals `value`,
/// reporting a requested value through `on_value_change` — a **controlled**
/// component (see the [module docs](self)).
pub fn tabs<State: 'static, F: Fn(&mut State, String) + 'static>(
    value: impl Into<String>,
    tabs: Vec<TabsTab<State>>,
    on_value_change: F,
) -> TabsView<State> {
    TabsView {
        value: value.into(),
        tabs,
        variant: TabsVariant::default(),
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> TabsView<State> {
    /// Pick the strip's `variant`.
    pub fn variant(mut self, variant: TabsVariant) -> Self {
        self.variant = variant;
        self
    }
}

/// The resolved tab palette.
struct TabsColors {
    /// The pill/segment strip (`bg-card`).
    strip: Color,
    /// The indicator (`bg-primary`, which beUI aliases onto the ink colour).
    indicator: Color,
    /// The active label over a filled indicator (`text-primary-foreground`).
    on_indicator: Color,
    /// The active label with nothing painted under it (`text-foreground`).
    active_ink: Color,
    /// An inactive label (`text-muted-foreground`).
    inactive_ink: Color,
    /// The underline variant's hairline rule (`border-border`).
    rule: Color,
}

/// Resolve the palette, falling back to the vendored **light** table with no
/// theme threaded — the unthemed posture every component in this catalog takes.
fn resolve_colors(theme: Option<&Theme>) -> TabsColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            TabsColors {
                strip: scheme.surface_container,
                indicator: scheme.primary,
                on_indicator: scheme.on_primary,
                active_ink: scheme.on_surface,
                inactive_ink: scheme.on_surface_variant,
                rule: scheme.outline_variant,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            TabsColors {
                strip: p.card,
                indicator: p.primary,
                on_indicator: p.primary_foreground,
                active_ink: p.foreground,
                inactive_ink: p.muted_foreground,
                rule: p.border,
            }
        }
    }
}

/// The trigger label style: the theme's Geist stack at `text-sm`/`font-medium`,
/// or the bundled sans stack unthemed.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, Color::BLACK)
    }
}

/// Which way an arrow key moves along the strip, or `None`.
fn arrow_step(key: &KeyEvent) -> Option<isize> {
    match &key.key {
        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => Some(1),
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => Some(-1),
        _ => None,
    }
}

/// Linear interpolation between two rects, `t` **unclamped** so a spring's
/// overshoot carries into the geometry.
fn lerp_rect(from: Rect, to: Rect, t: f64) -> Rect {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x0, to.x0),
        lerp(from.y0, to.y0),
        lerp(from.x1, to.x1),
        lerp(from.y1, to.y1),
    )
}

/// One retained trigger: its identity plus the geometry layout resolved.
struct TriggerEntry {
    value: String,
    label: String,
    run: LabelRun,
    disabled: bool,
    /// Local x of the trigger's left edge (filled at layout).
    x: f64,
    /// The trigger's width incl. padding (filled at layout).
    width: f64,
}

impl TriggerEntry {
    fn from_tab<State: 'static>(tab: &TabsTab<State>) -> Self {
        Self {
            value: tab.value.clone(),
            label: tab.label.clone(),
            run: LabelRun::new(tab.label.clone()),
            disabled: tab.disabled,
            x: 0.0,
            width: 0.0,
        }
    }
}

impl<State: 'static> View<State> for TabsView<State> {
    type Element = TabsWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TabsWidget {
        let mut widget = TabsWidget {
            triggers: self.tabs.iter().map(TriggerEntry::from_tab).collect(),
            panels: self
                .tabs
                .iter()
                .map(|tab| build_child(&tab.content, ctx))
                .collect(),
            value: self.value.clone(),
            variant: self.variant,
            focused_tab: 0,
            hovered: None,
            captured: None,
            trigger_height: 0.0,
            strip_height: 0.0,
            indicator_from: None,
            indicator_shown: None,
            indicator_started: None,
            content_started: None,
            content_playing: false,
            on_value_change: erase_callback_arg(&self.on_value_change),
        };
        widget.focused_tab = widget.active().unwrap_or(0);
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut TabsWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;

        if prev.tabs.len() != self.tabs.len() {
            element.triggers = self.tabs.iter().map(TriggerEntry::from_tab).collect();
            element.captured = None;
            element.hovered = None;
            element.indicator_from = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, tab) in element.triggers.iter_mut().zip(self.tabs.iter()) {
                if entry.label != tab.label {
                    entry.run.set_content(tab.label.clone());
                    entry.label = tab.label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.value != tab.value || entry.disabled != tab.disabled {
                    entry.value = tab.value.clone();
                    entry.disabled = tab.disabled;
                    flags |= ChangeFlags::PAINT;
                }
            }
        }

        if prev.value != self.value {
            // The app confirmed a new value: adopt it, move the roving focus,
            // and launch the indicator from wherever it is *displaying* rather
            // than from the trigger it nominally belonged to — a retarget
            // mid-travel must not jump backwards first.
            element.indicator_from = element.indicator_shown.or_else(|| element.target_rect());
            element.indicator_started = None;
            element.value = self.value.clone();
            if let Some(index) = element.active() {
                element.focused_tab = index;
            }
            element.content_started = None;
            element.content_playing = true;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            element.indicator_from = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= rebuild_children(
            &prev.tabs,
            &self.tabs,
            &mut element.panels,
            ctx,
            |tab: &TabsTab<State>| &tab.content,
            |_| None::<ChildKey>,
        );
        element.focused_tab = element
            .focused_tab
            .min(element.triggers.len().saturating_sub(1));
        flags
    }

    fn teardown(&self, element: &mut TabsWidget, ctx: &mut BuildCtx<'_>) {
        for (tab, pod) in self.tabs.iter().zip(element.panels.iter_mut()) {
            teardown_child(&tab.content, pod, ctx);
        }
    }
}

/// The retained widget for a [`TabsView`].
pub struct TabsWidget {
    triggers: Vec<TriggerEntry>,
    /// One panel per tab; only the active one is laid out, painted, routed or
    /// published.
    panels: Vec<ChildPod>,
    /// The app-confirmed active value (source of truth, adopted on `rebuild`).
    value: String,
    variant: TabsVariant,
    /// The trigger the roving focus sits on.
    focused_tab: usize,
    /// The latched hovered trigger, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The trigger a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<usize>,
    /// The trigger box height layout resolved.
    trigger_height: f64,
    /// The whole strip's height layout resolved.
    strip_height: f64,
    /// The rect the indicator's current travel started from.
    indicator_from: Option<Rect>,
    /// The rect the indicator painted last frame — the retarget origin.
    indicator_shown: Option<Rect>,
    /// The frame the current travel was first painted at.
    indicator_started: Option<FrameTime>,
    /// The frame the panel entrance was first painted at.
    content_started: Option<FrameTime>,
    /// Whether a panel entrance is staged (cleared when it settles).
    content_playing: bool,
    on_value_change: frust::authoring::ErasedArgCallback<String>,
}

impl TabsWidget {
    /// The active tab's index, if any tab's value matches.
    fn active(&self) -> Option<usize> {
        self.triggers.iter().position(|t| t.value == self.value)
    }

    /// Whether trigger `index` can be interacted with.
    fn enabled(&self, index: usize) -> bool {
        self.triggers.get(index).is_some_and(|t| !t.disabled)
    }

    /// Trigger `index`'s local box.
    fn trigger_rect(&self, index: usize) -> Option<Rect> {
        self.triggers.get(index).map(|t| {
            Rect::from_origin_size(
                Point::new(t.x, self.variant.padding()),
                Size::new(t.width, self.trigger_height),
            )
        })
    }

    /// Where the indicator belongs for trigger `index`: the whole trigger box in
    /// the filled variants, a hairline on the strip's bottom edge in the
    /// underline one.
    fn indicator_rect_for(&self, index: usize) -> Option<Rect> {
        let rect = self.trigger_rect(index)?;
        Some(match self.variant {
            TabsVariant::Pill | TabsVariant::Segment => rect,
            TabsVariant::Underline => Rect::new(
                rect.x0,
                self.strip_height - TABS_INDICATOR_THICKNESS,
                rect.x1,
                self.strip_height,
            ),
        })
    }

    /// The indicator's resting target, if a tab is active.
    fn target_rect(&self) -> Option<Rect> {
        self.active().and_then(|i| self.indicator_rect_for(i))
    }

    /// The strip's own width: triggers, gaps and the strip padding.
    fn strip_width(&self) -> f64 {
        let content: f64 = self.triggers.iter().map(|t| t.width).sum();
        let gaps = self.variant.gap() * self.triggers.len().saturating_sub(1) as f64;
        content + gaps + self.variant.padding() * 2.0
    }

    /// The trigger under a widget-local `pos`, if any (the strip band only).
    fn hit_trigger(&self, pos: Point) -> Option<usize> {
        (0..self.triggers.len())
            .find(|index| self.trigger_rect(*index).is_some_and(|r| r.contains(pos)))
    }

    /// The next enabled trigger `step` places from `from`, wrapping.
    fn step_enabled(&self, from: usize, step: isize) -> Option<usize> {
        let len = self.triggers.len();
        if len == 0 {
            return None;
        }
        let mut index = from;
        for _ in 0..len {
            index = (index as isize + step).rem_euclid(len as isize) as usize;
            if self.enabled(index) {
                return Some(index);
            }
        }
        None
    }

    /// Report trigger `index`'s value (never assigning it to `self.value`).
    fn request_value(&mut self, ctx: &mut EventCtx, index: usize) {
        if let Some(trigger) = self.triggers.get(index) {
            let value = trigger.value.clone();
            (self.on_value_change)(ctx, value);
        }
    }

    /// Advance the indicator to `now`, returning the rect to paint and whether
    /// the travel is still running.
    fn advance_indicator(&mut self, now: FrameTime, reduce_motion: bool) -> (Option<Rect>, bool) {
        let Some(target) = self.target_rect() else {
            self.indicator_shown = None;
            return (None, false);
        };
        let Some(from) = self.indicator_from.filter(|_| !reduce_motion) else {
            self.indicator_from = None;
            self.indicator_started = None;
            self.indicator_shown = Some(target);
            return (Some(target), false);
        };

        let ramp = Ramp::spring(TABS_INDICATOR_SPRING);
        let started = *self.indicator_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.indicator_from = None;
            self.indicator_started = None;
            self.indicator_shown = Some(target);
            return (Some(target), false);
        }
        // Raw progress: passing the target rect and settling back is the point.
        let shown = lerp_rect(from, target, ramp.progress(elapsed));
        self.indicator_shown = Some(shown);
        (Some(shown), true)
    }

    /// Ink every trigger label: the on-indicator ink over a filled pill, the
    /// full ink for the active underline trigger and for whatever the pointer is
    /// over (`hover:text-foreground`), the dimmed token otherwise.
    fn paint_labels(&self, origin: Point, colors: &TabsColors, scene: &mut dyn PaintScene) {
        let active = self.active();
        for (index, trigger) in self.triggers.iter().enumerate() {
            let dimmed = !self.enabled(index);
            let ink = match (self.variant, active == Some(index)) {
                (TabsVariant::Pill | TabsVariant::Segment, true) => colors.on_indicator,
                (_, true) => colors.active_ink,
                _ if self.hovered == Some(index) && !dimmed => colors.active_ink,
                _ => colors.inactive_ink,
            };
            let label = trigger.run.size();
            let box_top = match self.variant {
                TabsVariant::Underline => 0.0,
                _ => self.variant.padding(),
            };
            let label_origin = Point::new(
                origin.x + trigger.x + (trigger.width - label.width) / 2.0,
                origin.y + box_top + (self.trigger_height - label.height) / 2.0,
            );
            trigger.run.paint(
                label_origin,
                style::disabled_tint(ink, dimmed, style::DISABLED_OPACITY),
                scene,
            );
        }
    }

    /// Advance the panel entrance to `now`, returning `(alpha, rise, running)`.
    fn advance_content(&mut self, now: FrameTime, reduce_motion: bool) -> (f32, f64, bool) {
        if !self.content_playing {
            return (1.0, 0.0, false);
        }
        if reduce_motion {
            self.content_playing = false;
            self.content_started = None;
            return (1.0, 0.0, false);
        }
        let started = *self.content_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if TABS_CONTENT_TIMING.is_settled(elapsed) {
            self.content_playing = false;
            self.content_started = None;
            return (1.0, 0.0, false);
        }
        let t = TABS_CONTENT_TIMING.progress_clamped(elapsed);
        (t as f32, (1.0 - t) * TABS_CONTENT_RISE, true)
    }
}

impl Widget for TabsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let text_style = label_style(Theme::from_layout_ctx(ctx));
        let padding = self.variant.padding();
        let gap = self.variant.gap();
        let padding_x = self.variant.trigger_padding_x();

        let mut label_height: f64 = 0.0;
        let mut x = padding;
        for trigger in &mut self.triggers {
            let label = trigger.run.layout(ctx, &text_style);
            label_height = label_height.max(label.height);
            trigger.x = x;
            trigger.width = label.width + padding_x * 2.0;
            x += trigger.width + gap;
        }

        match self.variant {
            TabsVariant::Pill | TabsVariant::Segment => {
                self.trigger_height = label_height + TABS_TRIGGER_PADDING_Y * 2.0;
                self.strip_height = self.trigger_height + padding * 2.0;
            }
            TabsVariant::Underline => {
                self.trigger_height = label_height.max(TABS_UNDERLINE_MIN_HEIGHT);
                self.strip_height = self.trigger_height;
            }
        }

        // Only the active panel is measured; a hidden one keeps whatever
        // geometry it last had and is never painted or routed to.
        let panel_size = match self.active().and_then(|index| self.panels.get_mut(index)) {
            Some(pod) => {
                let max = Size::new(
                    bc.max().width,
                    (bc.max().height - self.strip_height - TABS_CONTENT_GAP).max(0.0),
                );
                let size = pod.layout_child(ctx, &BoxConstraints::new(Size::ZERO, max));
                pod.set_origin(Point::new(0.0, self.strip_height + TABS_CONTENT_GAP));
                size
            }
            None => Size::ZERO,
        };

        let width = self.strip_width().max(panel_size.width);
        let height = if panel_size.height > 0.0 {
            self.strip_height + TABS_CONTENT_GAP + panel_size.height
        } else {
            self.strip_height
        };
        bc.constrain(Size::new(width, height))
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
        let origin = ctx.origin();
        let active = self.active();
        let strip_width = self.strip_width();

        let (indicator, indicator_running) =
            self.advance_indicator(ctx.frame_time(), reduce_motion);
        let (content_alpha, content_rise, content_running) =
            self.advance_content(ctx.frame_time(), reduce_motion);

        // The strip: a `card` fill in the filled variants, a hairline rule along
        // the bottom edge in the underline one (`border-b border-border`).
        if self.variant.has_strip_fill() {
            scene.fill_rounded_rect(
                origin,
                Size::new(strip_width, self.strip_height),
                style::resolve_radius(self.variant.radius(), strip_width, self.strip_height),
                colors.strip,
            );
        } else {
            scene.fill_rect(
                Point::new(origin.x, origin.y + self.strip_height - style::BORDER_WIDTH),
                Size::new(strip_width, style::BORDER_WIDTH),
                colors.rule,
            );
        }

        // The indicator, under the labels.
        if let Some(rect) = indicator {
            let indicator_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let indicator_size = Size::new(rect.width().max(0.0), rect.height().max(0.0));
            match self.variant {
                TabsVariant::Underline => {
                    scene.fill_rect(indicator_origin, indicator_size, colors.indicator);
                }
                TabsVariant::Pill | TabsVariant::Segment => scene.fill_rounded_rect(
                    indicator_origin,
                    indicator_size,
                    style::resolve_radius(
                        self.variant.radius(),
                        indicator_size.width,
                        indicator_size.height,
                    ),
                    colors.indicator,
                ),
            }
        }

        self.paint_labels(origin, &colors, scene);

        // The panel last, so its content sits above the strip chrome, staged by
        // the entrance ramp (opacity plus a short rise).
        if let Some(pod) = active.and_then(|index| self.panels.get_mut(index)) {
            if content_running {
                scene.push_layer(origin, ctx.size(), content_alpha);
                scene.push_transform(Affine::translate((0.0, content_rise)));
                pod.paint_child(ctx, scene);
                scene.pop_transform();
                scene.pop_layer();
            } else {
                pod.paint_child(ctx, scene);
            }
        }

        // The indicator travels *within* the already-measured strip and the
        // panel entrance is paint-only, so a bare frame request covers both.
        if indicator_running || content_running {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // A broadcast is not user input: every panel gets it unconditionally,
        // ahead of everything else, and it is never consumed.
        if event.is_broadcast() {
            for pod in &mut self.panels {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }

        // The active panel is routed FIRST, which is what makes the strip's own
        // hover claim below a fallback rather than a pre-emption.
        if let Some(index) = self.active()
            && let Some(pod) = self.panels.get_mut(index)
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }

        match event {
            InputEvent::Key(key) => {
                if let Some(step) = arrow_step(key) {
                    let Some(next) = self.step_enabled(self.focused_tab, step) else {
                        return EventResult::Ignored;
                    };
                    self.focused_tab = next;
                    ctx.request_redraw();
                    self.request_value(ctx, next);
                    return EventResult::Handled;
                }
                if is_activation_key(key) && self.enabled(self.focused_tab) {
                    let index = self.focused_tab;
                    self.request_value(ctx, index);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_trigger(p.position).filter(|i| self.enabled(*i))
                    else {
                        return EventResult::Ignored;
                    };
                    self.captured = Some(index);
                    self.focused_tab = index;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_none() {
                        let over = self
                            .hit_trigger(p.position)
                            .filter(|index| self.enabled(*index));
                        if over.is_some() {
                            ctx.claim_hover();
                            ctx.set_cursor(style::ACTIVE_CURSOR);
                        }
                        if self.hovered != over {
                            self.hovered = over;
                            ctx.request_redraw();
                        }
                        return EventResult::Ignored;
                    }
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    let Some(armed) = self.captured.take() else {
                        return EventResult::Ignored;
                    };
                    if self.hit_trigger(p.position) == Some(armed) {
                        self.request_value(ctx, armed);
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
                for (index, trigger) in self.triggers.iter().enumerate() {
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(trigger.label.as_str());
                        node.set_selected(active == Some(index));
                        if trigger.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
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

    visit_children!(panels);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{
        BezPath, Brush, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::text;
    use std::any::Any;

    /// Records everything the widget paints.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        inks: Vec<Color>,
        layers: Vec<f32>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.layers.push(alpha);
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
    struct Picked {
        last: Option<String>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn tab_list() -> Vec<TabsTab<Picked>> {
        vec![
            tabs_tab("overview", "Overview", text("overview panel")),
            tabs_tab("activity", "Activity and more", text("activity panel")),
            tabs_tab("settings", "Settings", text("settings panel")).disabled(true),
        ]
    }

    fn view(value: &str, variant: TabsVariant) -> TabsView<Picked> {
        tabs::<Picked, _>(value, tab_list(), |s: &mut Picked, v: String| {
            s.last = Some(v);
            s.count += 1;
        })
        .variant(variant)
    }

    fn build(value: &str, variant: TabsVariant) -> TabsWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(value, variant), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut TabsWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(600.0, 400.0)),
        )
    }

    fn laid_out(value: &str, variant: TabsVariant) -> (TabsWidget, Size) {
        let mut w = build(value, variant);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut TabsWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    /// Re-run the view with a new value, exactly as a rebuild would.
    fn retarget(w: &mut TabsWidget, from: &str, to: &str, variant: TabsVariant) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(to, variant), &view(from, variant), w, &mut ctx);
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

    fn dispatch(w: &mut TabsWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The strip lays its triggers out left to right, gapped by the variant's
    /// own rule — flush in the segment variant, `gap-1` otherwise.
    #[test]
    fn the_strip_lays_triggers_out_in_order_with_the_variant_gap() {
        let (pill, _) = laid_out("overview", TabsVariant::Pill);
        let first = pill.trigger_rect(0).unwrap();
        let second = pill.trigger_rect(1).unwrap();
        assert_eq!(first.x0, TABS_PILL_PADDING);
        assert_eq!(second.x0, first.x1 + TABS_LIST_GAP);
        assert!(
            second.width() > first.width(),
            "the longer label takes the wider trigger"
        );

        let (segment, _) = laid_out("overview", TabsVariant::Segment);
        assert_eq!(
            segment.trigger_rect(1).unwrap().x0,
            segment.trigger_rect(0).unwrap().x1,
            "the segment variant packs its triggers flush"
        );
    }

    /// The underline variant honours upstream's own 44px tap-target floor and
    /// puts its bar on the strip's bottom edge.
    #[test]
    fn the_underline_trigger_keeps_the_upstream_tap_target_floor() {
        let (w, _) = laid_out("overview", TabsVariant::Underline);
        assert!(w.trigger_height >= TABS_UNDERLINE_MIN_HEIGHT);
        let bar = w.indicator_rect_for(0).unwrap();
        assert_eq!(bar.height(), TABS_INDICATOR_THICKNESS);
        assert_eq!(bar.y1, w.strip_height);
    }

    /// At rest the indicator sits exactly on the active trigger, and no frame is
    /// owed for it.
    #[test]
    fn the_indicator_rests_on_the_active_trigger() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        let (rec, needs_frame) = paint_at(&mut w, size, None, 0.0);
        assert!(!needs_frame, "a settled tab set animates nothing");
        let target = w.trigger_rect(0).unwrap();
        assert!(
            rec.rrects
                .iter()
                .any(|(o, s, _, c)| *o == Point::new(target.x0, target.y0)
                    && s.width == target.width()
                    && *c == crate::BEUI_LIGHT.primary),
            "the indicator paints on the active trigger: {:?}",
            rec.rrects
        );
    }

    /// The retarget property: a value change launches the indicator from where
    /// it was, it is strictly between the two triggers mid-flight, and it lands
    /// exactly on the new one.
    #[test]
    fn the_indicator_springs_from_the_old_trigger_to_the_new_one() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        paint_at(&mut w, size, None, 0.0);
        let from = w.trigger_rect(0).unwrap();
        let to = w.trigger_rect(1).unwrap();

        retarget(&mut w, "overview", "activity", TabsVariant::Pill);
        layout(&mut w);

        // The first frame of the travel is still at the old rect...
        let (_, needs_frame) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_frame, "a travelling indicator owes frames");
        let start = w.indicator_shown.unwrap();
        assert!((start.x0 - from.x0).abs() < 0.01, "started at {start:?}");

        // ...mid-flight it is somewhere between the two, stretching as it goes...
        paint_at(&mut w, size, None, 180.0);
        let mid = w.indicator_shown.unwrap();
        assert!(
            mid.x0 > from.x0 && mid.x0 < to.x0,
            "mid-flight {mid:?} is not between {from:?} and {to:?}"
        );
        assert!(
            mid.width() > from.width() && mid.width() < to.width(),
            "the width stretches with the travel: {mid:?}"
        );

        // ...and past the spring's settle time it is exactly on the target and
        // stops asking for frames.
        let (_, still) = paint_at(&mut w, size, None, 100.0 + 5_000.0);
        assert!(!still, "a settled travel owes no frame");
        assert_eq!(w.indicator_shown.unwrap(), to);
    }

    /// A second change mid-travel re-launches from the *displayed* rect, not
    /// from the trigger the indicator nominally belonged to — otherwise the pill
    /// would jump backwards before setting off again.
    #[test]
    fn a_retarget_mid_travel_starts_from_where_the_indicator_is() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        paint_at(&mut w, size, None, 0.0);
        retarget(&mut w, "overview", "settings", TabsVariant::Pill);
        layout(&mut w);
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 160.0);
        let mid = w.indicator_shown.unwrap();

        retarget(&mut w, "settings", "activity", TabsVariant::Pill);
        layout(&mut w);
        assert_eq!(w.indicator_from, Some(mid));
    }

    /// `reduce_motion` snaps the indicator onto its target and stages no panel
    /// entrance — upstream's `{ duration: 0 }` arm.
    #[test]
    fn reduce_motion_snaps_the_indicator_and_the_panel_entrance() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;

        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        paint_at(&mut w, size, Some(&theme), 0.0);
        retarget(&mut w, "overview", "activity", TabsVariant::Pill);
        layout(&mut w);
        let (rec, needs_frame) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(!needs_frame, "reduced motion owes no frame");
        assert_eq!(w.indicator_shown, w.trigger_rect(1));
        assert!(
            rec.layers.is_empty() && rec.transforms.is_empty(),
            "no staged panel entrance under reduced motion"
        );
    }

    /// The panel entrance fades and rises, then settles to a plain paint.
    #[test]
    fn the_panel_entrance_fades_and_rises_then_settles() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        paint_at(&mut w, size, None, 0.0);
        retarget(&mut w, "overview", "activity", TabsVariant::Pill);
        layout(&mut w);

        let (rec, _) = paint_at(&mut w, size, None, 100.0);
        assert_eq!(rec.layers.len(), 1, "the entering panel is composited");
        assert!(rec.layers[0] < 1.0, "it starts transparent");
        assert_eq!(
            rec.transforms[0],
            Affine::translate((0.0, TABS_CONTENT_RISE)),
            "and starts a full rise below its resting place"
        );

        let (settled, _) = paint_at(&mut w, size, None, 100.0 + 200.0);
        assert!(
            settled.layers.is_empty(),
            "a settled panel paints without a layer"
        );
        assert!(!w.content_playing);
    }

    /// A press reports the pressed tab's value and never assigns it locally —
    /// the controlled contract.
    #[test]
    fn a_press_reports_the_value_without_adopting_it() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        let mut state = Picked::default();
        let at = w.trigger_rect(1).unwrap().center();

        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.last.as_deref(), Some("activity"));
        assert_eq!(w.value, "overview", "the widget never writes its own value");
    }

    /// A disabled trigger is inert to the pointer and skipped by the arrows.
    #[test]
    fn a_disabled_trigger_is_skipped_by_pointer_and_keyboard() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        let mut state = Picked::default();
        let at = w.trigger_rect(2).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.count, 0, "a disabled trigger reports nothing");

        // From the second tab, forward wraps past the disabled third onto the
        // first rather than landing on it.
        w.focused_tab = 1;
        dispatch(&mut w, size, &key_event(NamedKey::ArrowRight), &mut state);
        assert_eq!(state.last.as_deref(), Some("overview"));
        assert_eq!(w.focused_tab, 0);
    }

    /// Arrow keys activate as they move (automatic activation), and `Enter`
    /// reports the focused tab.
    #[test]
    fn the_arrows_move_the_roving_focus_and_activate() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        let mut state = Picked::default();
        dispatch(&mut w, size, &key_event(NamedKey::ArrowRight), &mut state);
        assert_eq!(w.focused_tab, 1);
        assert_eq!(state.last.as_deref(), Some("activity"));

        dispatch(&mut w, size, &key_event(NamedKey::Enter), &mut state);
        assert_eq!(state.count, 2, "Enter reports the focused tab again");
    }

    /// The filled variants ink the active label with `primary-foreground`; the
    /// underline one leaves it `foreground`, because nothing is painted under
    /// it.
    #[test]
    fn the_active_label_ink_follows_the_variant() {
        let p = crate::BEUI_LIGHT;
        let (mut pill, size) = laid_out("overview", TabsVariant::Pill);
        let (rec, _) = paint_at(&mut pill, size, None, 0.0);
        assert_eq!(rec.inks[0], p.primary_foreground);
        assert_eq!(rec.inks[1], p.muted_foreground);

        let (mut underline, size) = laid_out("overview", TabsVariant::Underline);
        let (rec, _) = paint_at(&mut underline, size, None, 0.0);
        assert_eq!(rec.inks[0], p.foreground);
    }

    /// A move over an enabled trigger latches it as hovered, a move over the
    /// disabled one does not, and the latch self-corrects against the
    /// paint-time hover read (which reports no link here, so it clears).
    #[test]
    fn the_hover_latch_tracks_enabled_triggers_and_self_corrects() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        let mut state = Picked::default();
        let over = w.trigger_rect(1).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Move, over), &mut state);
        assert_eq!(w.hovered, Some(1));

        let disabled = w.trigger_rect(2).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, disabled),
            &mut state,
        );
        assert_eq!(w.hovered, None, "a disabled trigger never latches");

        w.hovered = Some(1);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(
            w.hovered, None,
            "no hover link on the path clears the latch"
        );
    }

    /// With the latch standing, the hovered inactive label paints the full ink
    /// (`hover:text-foreground`) instead of the dimmed token.
    #[test]
    fn a_hovered_inactive_label_paints_the_full_ink() {
        let (mut w, size) = laid_out("overview", TabsVariant::Pill);
        let (resting, _) = paint_at(&mut w, size, None, 0.0);
        assert_eq!(resting.inks[1], crate::BEUI_LIGHT.muted_foreground);

        // Ink the labels through the same routine `paint` calls: a full paint
        // pass would first self-correct the latch away against a test context
        // that reports no hover link at all.
        let mut rec = Recorder::default();
        w.hovered = Some(1);
        w.paint_labels(Point::ZERO, &resolve_colors(None), &mut rec);
        assert_eq!(rec.inks[1], crate::BEUI_LIGHT.foreground);
    }

    /// The strip publishes one `Tab` node per trigger, carrying its selection
    /// and its disabled state.
    #[test]
    fn semantics_publish_one_tab_node_per_trigger() {
        let mut root = frust_core::RenderRoot::new();
        let mut state = Picked::default();
        let mut tcx = TextContext::new();
        let mut logic = move |_s: &mut Picked| view("activity", TabsVariant::Pill);
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(600.0, 400.0), &mut tcx as &mut dyn Any);

        let update: SemanticsUpdate = root.semantics();
        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3, "one node per trigger");
        assert_eq!(
            tabs.iter()
                .filter(|(_, n)| n.is_selected() == Some(true))
                .count(),
            1,
            "exactly one selected tab"
        );
    }
}
