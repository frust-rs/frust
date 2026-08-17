//! Ports shadcn/ui's **Tabs** (`Tabs` + `TabsList` + `TabsTrigger` +
//! `TabsContent`) from `tmp/ui/apps/v4/registry/new-york-v4/ui/tabs.tsx` (rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17).
//!
//! Four upstream components collapse into one widget: the list, its triggers and
//! the active panel are one interactive unit (a trigger's look depends on the
//! list's variant, and arrow-key traversal spans the whole strip), so the port
//! keeps them together and takes the panels as child views.
//!
//! | class | here |
//! |---|---|
//! | root `flex gap-2 data-[orientation=horizontal]:flex-col` | the list, [`TABS_ROOT_GAP`], then the active panel |
//! | list `h-9 p-[3px] rounded-lg` | [`TABS_LIST_HEIGHT`]/[`TABS_LIST_PADDING`], `ShadcnRadius::lg` |
//! | list `variant=default`: `bg-muted` | [`TabsVariant::Default`]'s pill strip |
//! | list `variant=line`: `gap-1 bg-transparent rounded-none` | [`TabsVariant::Line`] + [`TABS_LINE_GAP`] |
//! | trigger `h-[calc(100%-1px)] px-2 py-1 text-sm font-medium rounded-md` | [`TABS_TRIGGER_HEIGHT_INSET`], [`TABS_TRIGGER_PADDING_X`] |
//! | trigger `text-foreground/60` → `hover:text-foreground` | [`TABS_INACTIVE_INK_ALPHA`], hover swapping to the full ink |
//! | trigger `dark:text-muted-foreground` | dark mode's own inactive ink (opaque, not an alpha) |
//! | active `data-[state=active]:bg-background` + `shadow-sm` (default variant) | the raised card look |
//! | active `dark:bg-input/30 dark:border-input` | dark mode's filled+bordered active pill |
//! | active `after:h-0.5 after:bg-foreground` (line variant) | [`TABS_INDICATOR_THICKNESS`] at [`TABS_INDICATOR_OFFSET`] below the trigger |
//! | `focus-visible:border-ring` + `ring-[3px] ring-ring/50` | [`style::focus_border`] + [`style::draw_focus_ring`] |
//!
//! # Horizontal only
//!
//! The source models a vertical orientation (`data-[orientation=vertical]`: the
//! list beside the panel, the indicator on the right edge); this port covers the
//! horizontal one and leaves the vertical geometry unported.
//!
//! # Controlled, with automatic activation
//!
//! `value` is a prop; a press, an arrow key and `Space`/`Enter` all *report* the
//! requested value through `on_value_change` and never write it locally. Arrow
//! keys move the roving focus **and** activate, which is Radix's default
//! `activationMode="automatic"` (and unlike `toggle_group`, whose arrows only
//! move focus).
//!
//! # The line indicator cross-fades
//!
//! The source gives each trigger its own `::after` bar and transitions its
//! *opacity* (`after:transition-opacity`), so a selection change fades one bar out
//! while the other fades in. That is what the single progress lane here drives:
//! the outgoing bar paints at `1 - t`, the incoming at `t`, on Tailwind's default
//! duration/easing ([`INDICATOR_DURATION`]/[`INDICATOR_CURVE`]). Under
//! `theme.motion.reduce_motion` the lane snaps and owes no frame. The default
//! variant's fill/shadow swap is instant — the source transitions it too, but a
//! fill cross-fade would need to paint both pills at once, and the shadow is what
//! actually reads.
//!
//! # A container: it claims hover *after* routing
//!
//! The panels are `ChildPod`s, so this widget really is a container, and the
//! ordering rule binds: every event is routed to the active panel **first**, and
//! only then does the list claim the hover link for the trigger under the pointer
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics). A broadcast goes to *every*
//! panel, active or not, ahead of everything else.
//!
//! Only the active panel is laid out, painted, routed and published to semantics —
//! the input-parity carve-out the semantics convention allows for a container that
//! gates input.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx,
    EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, View, Widget, any,
    build_child, erase_callback_arg, rebuild_children, route_event_single, teardown_child,
    text::{FontWeight, TextContext, TextLayout, TextStyle},
    visit_children,
};
use frust::{AnimationController, Brightness, ChildKey, Curve, FrameTime, Theme};

use crate::style;
use crate::tokens::ShadcnTokens;

/// The tab strip's height, in logical px (`h-9`).
pub const TABS_LIST_HEIGHT: f64 = style::HEIGHT_DEFAULT;

/// The strip's inner padding, in logical px (`p-[3px]` — an arbitrary value, so a
/// constant rather than a spacing step).
pub const TABS_LIST_PADDING: f64 = 3.0;

/// How much shorter than the strip's content box a trigger is, in logical px
/// (`h-[calc(100%-1px)]`).
pub const TABS_TRIGGER_HEIGHT_INSET: f64 = 1.0;

/// Horizontal padding per trigger, in logical px (`px-2`).
pub const TABS_TRIGGER_PADDING_X: f64 = 8.0;

/// The gap between triggers in the line variant, in logical px (`gap-1`); the
/// default variant packs them flush.
pub const TABS_LINE_GAP: f64 = 4.0;

/// The gap between the strip and the active panel, in logical px (`gap-2` on the
/// root).
pub const TABS_ROOT_GAP: f64 = 8.0;

/// The line variant's underline thickness, in logical px (`after:h-0.5`).
pub const TABS_INDICATOR_THICKNESS: f64 = 2.0;

/// How far below a trigger's own box the underline sits, in logical px
/// (`after:bottom-[-5px]`).
pub const TABS_INDICATOR_OFFSET: f64 = 5.0;

/// Alpha of an inactive trigger's label in light mode (`text-foreground/60`).
///
/// Dark mode does not use an alpha at all — it swaps the token
/// (`dark:text-muted-foreground`).
pub const TABS_INACTIVE_INK_ALPHA: f32 = 0.60;

/// Alpha of the active pill's fill in dark mode (`dark:bg-input/30`).
const DARK_ACTIVE_FILL_ALPHA: f32 = 0.30;

/// The indicator cross-fade's duration — Tailwind's
/// `--default-transition-duration` (150ms), which is what the source's bare
/// `transition-opacity` resolves to.
const INDICATOR_DURATION: Duration = Duration::from_millis(150);
/// The cross-fade's easing — Tailwind's `--default-transition-timing-function`,
/// `cubic-bezier(0.4, 0, 0.2, 1)`.
const INDICATOR_CURVE: Curve = Curve::Cubic(0.4, 0.0, 0.2, 1.0);

/// Flattening tolerance for the active pill's border path.
const PATH_TOLERANCE: f64 = 0.1;

/// The color a label run is *shaped* with; never painted (each run is re-brushed
/// with the state's ink at paint time — see `toggle`'s module docs).
const SHAPING_INK: Color = Color::BLACK;

/// Unthemed fallback `--muted` (the default variant's strip).
const FALLBACK_MUTED: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed fallback `--foreground` (the active ink and the line indicator; an
/// inactive label is this at [`TABS_INACTIVE_INK_ALPHA`]).
const FALLBACK_FOREGROUND: Color = Color::from_rgb8(0x0A, 0x0A, 0x0A);
/// Unthemed fallback `--background` (the active pill).
///
/// The unthemed path is the source's **light** mode, which is why there is no
/// fallback for the dark-only `input` border/wash — those resolve from the theme
/// or not at all.
const FALLBACK_BACKGROUND: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

/// The tab strip's `variant` axis (`tabsListVariants`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabsVariant {
    /// `variant="default"`: a `muted` pill strip whose active tab reads as a
    /// raised card.
    #[default]
    Default,
    /// `variant="line"`: a transparent strip whose active tab is marked by an
    /// underline.
    Line,
}

/// One tab: the value it selects, its trigger label, and the panel shown while it
/// is active.
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
    /// Disable this tab: 50% opacity, inert, skipped by arrow-key traversal
    /// (`disabled:pointer-events-none disabled:opacity-50`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this tab selects.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A view-held, typed selection callback (erased on build).
type OnValueChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A declarative shadcn tab set. See the [module docs](self).
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
    /// The default variant's strip (`bg-muted`).
    strip: Color,
    /// The active pill (`bg-background`, or `input/30` in dark mode).
    active_fill: Color,
    /// The active pill's border: `Some` only in dark mode
    /// (`dark:data-[state=active]:border-input`).
    active_border: Option<Color>,
    /// The active label and the line indicator (`text-foreground`/
    /// `after:bg-foreground`).
    active_ink: Color,
    /// An inactive label: `foreground/60` in light mode, `muted-foreground` in
    /// dark.
    inactive_ink: Color,
}

/// Resolve the palette, falling back to the `neutral` preset's light values with
/// no theme threaded.
fn resolve_colors(theme: Option<&Theme>) -> TabsColors {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            match theme.brightness {
                Brightness::Dark => TabsColors {
                    strip: scheme.surface_container_highest,
                    active_fill: style::scale_alpha(scheme.outline_variant, DARK_ACTIVE_FILL_ALPHA),
                    active_border: Some(scheme.outline_variant),
                    active_ink: scheme.on_surface,
                    inactive_ink: scheme.on_surface_variant,
                },
                Brightness::Light => TabsColors {
                    strip: scheme.surface_container_highest,
                    active_fill: scheme.surface,
                    active_border: None,
                    active_ink: scheme.on_surface,
                    inactive_ink: style::with_alpha(scheme.on_surface, TABS_INACTIVE_INK_ALPHA),
                },
            }
        }
        None => TabsColors {
            strip: FALLBACK_MUTED,
            active_fill: FALLBACK_BACKGROUND,
            active_border: None,
            active_ink: FALLBACK_FOREGROUND,
            inactive_ink: style::with_alpha(FALLBACK_FOREGROUND, TABS_INACTIVE_INK_ALPHA),
        },
    }
}

/// The label style: the theme's (Inter) family at `text-sm`/`font-medium`, or the
/// bundled sans stack unthemed.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, SHAPING_INK)
    }
}

/// Whether `key` activates the focused tab.
fn is_activation_key(key: &KeyEvent) -> bool {
    match &key.key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == " ",
        _ => false,
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

/// A retained text run whose ink is applied at paint time.
struct LabelRun {
    content: String,
    layout: Option<TextLayout>,
    laid_out_style: Option<TextStyle>,
}

impl LabelRun {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            layout: None,
            laid_out_style: None,
        }
    }

    fn set_content(&mut self, content: impl Into<String>) {
        let content = content.into();
        if self.content != content {
            self.content = content;
            self.layout = None;
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if let Some(cached) = &self.layout
            && self.laid_out_style.as_ref() == Some(style)
        {
            return cached.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, style, None);
        let size = laid.size();
        self.layout = Some(laid);
        self.laid_out_style = Some(style.clone());
        size
    }

    fn size(&self) -> Size {
        self.layout.as_ref().map_or(Size::ZERO, |l| l.size())
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        if let Some(layout) = &self.layout {
            for mut run in layout.to_scene_runs(origin) {
                run.brush = Brush::Solid(color);
                scene.draw_glyph_run(run);
            }
        }
    }
}

/// One `0 → 1` cross-fade lane (the same shape `switch`'s travel lane carries).
struct Progress {
    ctrl: AnimationController,
    displayed: f64,
}

impl Progress {
    /// A lane already at its endpoint (a freshly-built tab set does not animate).
    fn settled() -> Self {
        Self {
            ctrl: AnimationController::new(INDICATOR_DURATION).with_curve(INDICATOR_CURVE),
            displayed: 1.0,
        }
    }

    /// Restart the fade from zero.
    fn restart(&mut self) {
        self.ctrl = AnimationController::new(INDICATOR_DURATION).with_curve(INDICATOR_CURVE);
        self.ctrl.forward();
        self.displayed = 0.0;
    }

    /// Land on the endpoint immediately (the `reduce_motion` path).
    fn snap(&mut self) {
        self.ctrl = AnimationController::new(INDICATOR_DURATION).with_curve(INDICATOR_CURVE);
        self.displayed = 1.0;
    }

    /// Advance to `now`, returning whether the lane is still in flight.
    fn advance(&mut self, now: FrameTime) -> bool {
        if self.ctrl.is_animating() {
            let animating = self.ctrl.advance(now);
            self.displayed = self.ctrl.value().clamp(0.0, 1.0);
            animating
        } else {
            self.displayed = 1.0;
            false
        }
    }

    fn value(&self) -> f64 {
        self.displayed
    }
}

/// One retained trigger: its value/label plus the geometry layout resolved.
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

impl<State: 'static> View<State> for TabsView<State> {
    type Element = TabsWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> TabsWidget {
        let triggers = self
            .tabs
            .iter()
            .map(|tab| TriggerEntry {
                value: tab.value.clone(),
                label: tab.label.clone(),
                run: LabelRun::new(tab.label.clone()),
                disabled: tab.disabled,
                x: 0.0,
                width: 0.0,
            })
            .collect();
        let panels = self
            .tabs
            .iter()
            .map(|tab| build_child(&tab.content, ctx))
            .collect();
        let mut widget = TabsWidget {
            triggers,
            panels,
            value: self.value.clone(),
            variant: self.variant,
            focused_tab: 0,
            hovered: None,
            captured: None,
            fade: Progress::settled(),
            fade_from: None,
            trigger_height: 0.0,
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

        // Trigger metadata is reconciled positionally alongside the panels (a tab
        // set is a fixed small list).
        if prev.tabs.len() != self.tabs.len() {
            element.triggers = self
                .tabs
                .iter()
                .map(|tab| TriggerEntry {
                    value: tab.value.clone(),
                    label: tab.label.clone(),
                    run: LabelRun::new(tab.label.clone()),
                    disabled: tab.disabled,
                    x: 0.0,
                    width: 0.0,
                })
                .collect();
            element.captured = None;
            element.hovered = None;
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
            // The app is the source of truth: adopt the confirmed value, move the
            // roving focus onto it, and start the indicator's cross-fade out of
            // the tab that is losing it.
            let outgoing = element.active();
            element.value = self.value.clone();
            if let Some(index) = element.active() {
                element.focused_tab = index;
            }
            element.fade_from = outgoing;
            element.fade.restart();
            // A different panel is shown, so the pass owes layout as well.
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
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
    /// published to semantics.
    panels: Vec<ChildPod>,
    /// The app-confirmed active value (source of truth; adopted on `rebuild`).
    value: String,
    variant: TabsVariant,
    /// The trigger the roving focus sits on.
    focused_tab: usize,
    /// The latched hovered trigger, self-corrected from `PaintCtx::is_hovered()`
    /// every paint.
    hovered: Option<usize>,
    /// The trigger a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<usize>,
    /// The line indicator's cross-fade lane.
    fade: Progress,
    /// The trigger the indicator is fading *out* of, if any.
    fade_from: Option<usize>,
    /// The trigger height layout resolved (the event pass hit-tests against it).
    trigger_height: f64,
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

    /// The inter-trigger gap: `gap-1` in the line variant, flush otherwise.
    fn gap(&self) -> f64 {
        match self.variant {
            TabsVariant::Default => 0.0,
            TabsVariant::Line => TABS_LINE_GAP,
        }
    }

    /// The trigger under a widget-local `pos`, if any (the strip band only).
    fn hit_trigger(&self, pos: Point) -> Option<usize> {
        self.triggers.iter().position(|t| {
            Rect::from_origin_size(
                Point::new(t.x, TABS_LIST_PADDING),
                Size::new(t.width, self.trigger_height),
            )
            .contains(pos)
        })
    }

    /// The next enabled trigger `step` places from `from`, wrapping.
    fn step_enabled(&self, from: usize, step: isize) -> Option<usize> {
        let len = self.triggers.len();
        if len == 0 {
            return None;
        }
        let mut index = from;
        for _ in 0..len {
            index = ((index as isize + step).rem_euclid(len as isize)) as usize;
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

    /// Set the latched hovered trigger, reporting whether it changed.
    fn set_hovered(&mut self, hovered: Option<usize>) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// The strip's own width: the triggers, their gaps, and the strip padding.
    fn strip_width(&self) -> f64 {
        let content: f64 = self.triggers.iter().map(|t| t.width).sum();
        let gaps = self.gap() * (self.triggers.len().saturating_sub(1)) as f64;
        content + gaps + TABS_LIST_PADDING * 2.0
    }

    /// The trigger `index`'s local box.
    fn trigger_rect(&self, index: usize) -> Option<Rect> {
        self.triggers.get(index).map(|t| {
            Rect::from_origin_size(
                Point::new(t.x, TABS_LIST_PADDING),
                Size::new(t.width, self.trigger_height),
            )
        })
    }
}

impl Widget for TabsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(Theme::from_layout_ctx(ctx));
        self.trigger_height =
            (TABS_LIST_HEIGHT - TABS_LIST_PADDING * 2.0 - TABS_TRIGGER_HEIGHT_INSET).max(0.0);

        let gap = self.gap();
        let mut x = TABS_LIST_PADDING;
        for trigger in &mut self.triggers {
            let label = trigger.run.layout(ctx, &style);
            trigger.x = x;
            trigger.width = label.width + TABS_TRIGGER_PADDING_X * 2.0;
            x += trigger.width + gap;
        }

        // Only the active panel is measured; a hidden one keeps whatever geometry
        // it last had and is never painted or routed to.
        let active = self.active();
        let panel_size = match active.and_then(|index| self.panels.get_mut(index)) {
            Some(pod) => {
                let max = Size::new(
                    bc.max().width,
                    (bc.max().height - TABS_LIST_HEIGHT - TABS_ROOT_GAP).max(0.0),
                );
                let size = pod.layout_child(ctx, &BoxConstraints::new(Size::ZERO, max));
                pod.set_origin(Point::new(0.0, TABS_LIST_HEIGHT + TABS_ROOT_GAP));
                size
            }
            None => Size::ZERO,
        };

        let width = self.strip_width().max(panel_size.width);
        let height = if panel_size.height > 0.0 {
            TABS_LIST_HEIGHT + TABS_ROOT_GAP + panel_size.height
        } else {
            TABS_LIST_HEIGHT
        };
        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The authoritative hover read: no link on this widget's path at all means
        // the latch is stale.
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let radii = ShadcnTokens::resolve_radius(None, theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let focused = ctx.has_focus();
        let origin = ctx.origin();
        let active = self.active();

        let owes_frame = if reduce_motion {
            self.fade.snap();
            false
        } else {
            self.fade.advance(ctx.frame_time())
        };
        let t = self.fade.value();

        // The strip: a `muted` pill in the default variant, nothing in the line
        // one (`bg-transparent rounded-none`).
        if self.variant == TabsVariant::Default {
            scene.fill_rounded_rect(
                origin,
                Size::new(self.strip_width(), TABS_LIST_HEIGHT),
                radii.lg,
                colors.strip,
            );
        }

        // The active pill (default variant only): `bg-background` + `shadow-sm`,
        // plus dark mode's `border-input`.
        if self.variant == TabsVariant::Default
            && let Some(index) = active
            && let Some(rect) = self.trigger_rect(index)
        {
            let pill_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let pill_size = Size::new(rect.width(), rect.height());
            let dimmed = !self.enabled(index);
            style::draw_shadow(
                scene,
                pill_origin,
                pill_size,
                radii.md,
                style::SHADOW_SM,
                theme,
            );
            scene.fill_rounded_rect(
                pill_origin,
                pill_size,
                radii.md,
                style::disabled_tint(colors.active_fill, dimmed),
            );
            if let Some(border) = colors.active_border {
                let inset = style::BORDER_WIDTH / 2.0;
                let outline = RoundedRect::from_rect(
                    Rect::from_origin_size(Point::ORIGIN, pill_size).inset(-inset),
                    (radii.md - inset).max(0.0),
                );
                scene.stroke_path(
                    pill_origin,
                    &Shape::to_path(&outline, PATH_TOLERANCE),
                    style::BORDER_WIDTH,
                    &Brush::Solid(style::disabled_tint(border, dimmed)),
                );
            }
        }

        // Labels: active ink for the active tab, the full ink under the pointer
        // (`hover:text-foreground`), the dimmed token otherwise.
        for (index, trigger) in self.triggers.iter().enumerate() {
            let dimmed = !self.enabled(index);
            let ink = if active == Some(index) || (self.hovered == Some(index) && !dimmed) {
                colors.active_ink
            } else {
                colors.inactive_ink
            };
            let label = trigger.run.size();
            let label_origin = Point::new(
                origin.x + trigger.x + (trigger.width - label.width) / 2.0,
                origin.y + TABS_LIST_PADDING + (self.trigger_height - label.height) / 2.0,
            );
            trigger
                .run
                .paint(label_origin, style::disabled_tint(ink, dimmed), scene);
        }

        // The line variant's underline, cross-fading between the outgoing and
        // incoming triggers.
        if self.variant == TabsVariant::Line {
            let bar_y = origin.y + TABS_LIST_PADDING + self.trigger_height + TABS_INDICATOR_OFFSET;
            let mut bar = |index: usize, alpha: f64| {
                if alpha <= 0.0 {
                    return;
                }
                if let Some(rect) = self.trigger_rect(index) {
                    scene.fill_rect(
                        Point::new(origin.x + rect.x0, bar_y),
                        Size::new(rect.width(), TABS_INDICATOR_THICKNESS),
                        style::scale_alpha(colors.active_ink, alpha as f32),
                    );
                }
            };
            if let Some(from) = self.fade_from.filter(|from| Some(*from) != active) {
                bar(from, 1.0 - t);
            }
            if let Some(index) = active {
                bar(index, t);
            }
        }

        // The focused trigger's ring, on top of the strip chrome.
        if focused && let Some(rect) = self.trigger_rect(self.focused_tab) {
            style::draw_focus_ring(
                scene,
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                Size::new(rect.width(), rect.height()),
                radii.md,
                style::ring_color(None, theme),
            );
        }

        // The active panel last, so its content sits above the strip chrome.
        if let Some(pod) = active.and_then(|index| self.panels.get_mut(index)) {
            pod.paint_child(ctx, scene);
        }

        // Paint-only cross-fade (no geometry moves), so a bare frame request.
        if owes_frame {
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

        // The active panel is routed FIRST — which is also what makes the list's
        // own hover claim below a fallback rather than a pre-emption.
        if let Some(index) = self.active()
            && let Some(pod) = self.panels.get_mut(index)
            && route_event_single(pod, ctx, event) == EventResult::Handled
        {
            return EventResult::Handled;
        }

        match event {
            InputEvent::Key(key) => {
                if let Some(step) = arrow_step(key) {
                    // Automatic activation: the arrow moves the roving focus and
                    // reports the new value in the same pass.
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
                    let Some(index) = self.hit_trigger(p.position) else {
                        return EventResult::Ignored;
                    };
                    if !self.enabled(index) {
                        return EventResult::Ignored;
                    }
                    self.captured = Some(index);
                    self.focused_tab = index;
                    ctx.capture_pointer();
                    ctx.request_focus();
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    if self.captured.is_none() {
                        // Claimed AFTER the panel routing above: a claiming
                        // descendant wins the pass, and this is the fallback that
                        // still gives the strip its own chrome.
                        let over = self
                            .hit_trigger(p.position)
                            .filter(|index| self.enabled(*index));
                        if over.is_some() {
                            ctx.claim_hover();
                            ctx.set_cursor(style::ACTIVE_CURSOR);
                        }
                        if self.set_hovered(over) {
                            ctx.request_redraw();
                        }
                        return EventResult::Ignored;
                    }
                    // Captured: re-ask so the shape survives a drag off the strip.
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
                    // Internal flags only.
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
        // Only the panel input can reach is published — the semantics convention's
        // input-parity carve-out for a container that gates input. No synthetic
        // `TabPanel` wrapper: it could only carry this whole widget's bounds, and a
        // node with the wrong bounds is worse than none.
        if let Some(pod) = active.and_then(|index| self.panels.get(index)) {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(panels);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{
        BezPath, EventOutcome, Modifiers, PointerButton, PointerEvent, SemanticsUpdate,
    };
    use frust::{CursorIcon, text};
    use std::any::Any;

    /// Records everything the widget paints.
    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        strokes: Vec<(Rect, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
        glyph_inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, brush: &Brush) {
            let bbox = path.bounding_box() + origin.to_vec2();
            let color = match brush {
                Brush::Solid(c) => *c,
                _ => Color::TRANSPARENT,
            };
            self.strokes.push((bbox, width, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
        fn draw_glyph_run(&mut self, run: frust::authoring::scene::GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.glyph_inks.push(color);
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
            tabs_tab("account", "Account", text("account panel")),
            tabs_tab("password", "Password", text("password panel")),
            tabs_tab("billing", "Billing", text("billing panel")).disabled(true),
        ]
    }

    fn view(value: &str, variant: TabsVariant) -> TabsView<Picked> {
        tabs::<Picked, _>(value, tab_list(), |s: &mut Picked, v: String| {
            s.last = Some(v);
            s.count += 1;
        })
        .variant(variant)
    }

    /// Build + lay out a tab set, returning it with its resolved size.
    fn laid_out(value: &str, variant: TabsVariant, theme: Option<&Theme>) -> (TabsWidget, Size) {
        let mut counter = 0u64;
        let mut w = View::<Picked>::build(&view(value, variant), &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, theme);
        (w, size)
    }

    fn layout(w: &mut TabsWidget, theme: Option<&Theme>) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx =
            LayoutCtx::with_resources(Some(&mut tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(400.0, 300.0)),
        )
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

    fn paint(w: &mut TabsWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        paint_at(w, size, theme, 0.0).0
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn key(key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn dispatch(
        w: &mut TabsWidget,
        state: &mut Picked,
        size: Size,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event)
    }

    /// The centre of trigger `index`.
    fn trigger_center(w: &TabsWidget, index: usize) -> Point {
        let rect = w.trigger_rect(index).expect("trigger exists");
        rect.center()
    }

    // ---- Layout -----------------------------------------------------------

    #[test]
    fn the_strip_is_h9_with_flush_triggers_and_the_panel_below_it() {
        let (w, size) = laid_out("account", TabsVariant::Default, None);
        assert_eq!(
            w.trigger_height,
            TABS_LIST_HEIGHT - TABS_LIST_PADDING * 2.0 - 1.0
        );
        assert_eq!(w.triggers[0].x, TABS_LIST_PADDING, "`p-[3px]`");
        assert_eq!(
            w.triggers[1].x,
            TABS_LIST_PADDING + w.triggers[0].width,
            "the default variant packs its triggers flush"
        );
        assert!(
            size.height > TABS_LIST_HEIGHT + TABS_ROOT_GAP,
            "the panel is below"
        );
        assert!(w.strip_width() > 0.0);
    }

    #[test]
    fn the_line_variant_gaps_its_triggers() {
        let (w, _) = laid_out("account", TabsVariant::Line, None);
        assert_eq!(w.gap(), TABS_LINE_GAP);
        assert_eq!(
            w.triggers[1].x,
            TABS_LIST_PADDING + w.triggers[0].width + TABS_LINE_GAP
        );
    }

    #[test]
    fn only_the_active_panel_is_laid_out() {
        let (w, _) = laid_out("password", TabsVariant::Default, None);
        assert_eq!(w.active(), Some(1));
        assert!(w.panels[1].size().height > 0.0, "the active panel measured");
        assert_eq!(w.panels[0].size(), Size::ZERO, "a hidden one did not");

        // An unmatched value leaves nothing active, and the strip is all there is.
        let mut counter = 0u64;
        let mut orphan = View::<Picked>::build(
            &view("nope", TabsVariant::Default),
            &mut BuildCtx::new(&mut counter),
        );
        let size = layout(&mut orphan, None);
        assert_eq!(orphan.active(), None);
        assert_eq!(size.height, TABS_LIST_HEIGHT);
    }

    // ---- Paint ------------------------------------------------------------

    #[test]
    fn the_default_variant_paints_a_muted_strip_and_a_raised_active_pill() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        let (mut w, size) = laid_out("account", TabsVariant::Default, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));

        assert_eq!(rec.rrects.len(), 2, "the strip, then the active pill");
        let (strip_origin, strip_size, strip_radius, strip) = rec.rrects[0];
        assert_eq!(strip_origin, Point::ZERO);
        assert_eq!(strip_size, Size::new(w.strip_width(), TABS_LIST_HEIGHT));
        assert_eq!(
            strip_radius,
            crate::ShadcnRadius::shadcn().lg,
            "`rounded-lg`"
        );
        assert_eq!(strip, scheme.surface_container_highest, "`bg-muted`");

        let (pill_origin, pill_size, pill_radius, pill) = rec.rrects[1];
        assert_eq!(
            pill_origin,
            Point::new(TABS_LIST_PADDING, TABS_LIST_PADDING)
        );
        assert_eq!(pill_size, Size::new(w.triggers[0].width, w.trigger_height));
        assert_eq!(
            pill_radius,
            crate::ShadcnRadius::shadcn().md,
            "`rounded-md`"
        );
        assert_eq!(pill, scheme.surface, "`bg-background`");
        assert_eq!(rec.shadows.len(), 1, "`shadow-sm` under the active pill");
        assert_eq!(rec.shadows[0].3, style::SHADOW_SM.std_dev);
        assert!(rec.strokes.is_empty(), "light mode strokes no pill border");

        // The active label reads in full ink, the inactive ones at 60%.
        assert_eq!(rec.glyph_inks[0], scheme.on_surface);
        assert_eq!(rec.glyph_inks[1].components[3], TABS_INACTIVE_INK_ALPHA);
        assert!(rec.rects.is_empty(), "no underline in the default variant");
    }

    #[test]
    fn dark_mode_gives_the_active_pill_an_input_wash_and_border() {
        let theme = crate::theme().with_brightness(Brightness::Dark);
        let scheme = theme.scheme();
        let (mut w, size) = laid_out("account", TabsVariant::Default, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        let input = scheme.outline_variant;
        assert!(
            (rec.rrects[1].3.components[3] - input.components[3] * DARK_ACTIVE_FILL_ALPHA).abs()
                < 1e-6,
            "`dark:bg-input/30`"
        );
        assert_eq!(rec.strokes.len(), 1, "`dark:border-input`");
        assert_eq!(rec.strokes[0].2, input);
        // Dark mode swaps the inactive token instead of using an alpha.
        assert_eq!(rec.glyph_inks[1], scheme.on_surface_variant);
    }

    #[test]
    fn the_line_variant_paints_an_underline_under_the_active_trigger_only() {
        let theme = crate::theme();
        let (mut w, size) = laid_out("password", TabsVariant::Line, Some(&theme));
        let rec = paint(&mut w, size, Some(&theme));
        assert!(rec.rrects.is_empty(), "`bg-transparent rounded-none`");
        assert!(rec.shadows.is_empty(), "and no `shadow-sm`");
        assert_eq!(rec.rects.len(), 1, "one bar");
        let (bar_origin, bar_size, bar) = rec.rects[0];
        let rect = w.trigger_rect(1).unwrap();
        assert_eq!(bar_origin.x, rect.x0);
        assert_eq!(
            bar_origin.y,
            TABS_LIST_PADDING + w.trigger_height + TABS_INDICATOR_OFFSET
        );
        assert_eq!(bar_size, Size::new(rect.width(), TABS_INDICATOR_THICKNESS));
        assert_eq!(bar, theme.scheme().on_surface, "`after:bg-foreground`");
    }

    #[test]
    fn a_selection_change_cross_fades_the_two_bars_and_settles() {
        let mut counter = 0u64;
        let prev = view("account", TabsVariant::Line);
        let mut w = View::<Picked>::build(&prev, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, None);
        // Settled at build: one bar, at full strength.
        let (rest, owes) = paint_at(&mut w, size, None, 0.0);
        assert!(!owes);
        assert_eq!(rest.rects.len(), 1);
        assert_eq!(rest.rects[0].2.components[3], 1.0);

        let next = view("password", TabsVariant::Line);
        View::<Picked>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, None);

        let (start, owes) = paint_at(&mut w, size, None, 0.0);
        assert!(owes, "an in-flight fade owes the next frame");
        // At `t == 0` the incoming bar is fully transparent and skipped, so the
        // outgoing one is all there is — still at full strength.
        assert_eq!(start.rects.len(), 1);
        assert_eq!(start.rects[0].2.components[3], 1.0);
        assert_eq!(start.rects[0].0.x, w.trigger_rect(0).unwrap().x0);

        let (mid, owes) = paint_at(&mut w, size, None, 75.0);
        assert!(owes);
        let (out_alpha, in_alpha) = (mid.rects[0].2.components[3], mid.rects[1].2.components[3]);
        assert!(out_alpha > 0.0 && out_alpha < 1.0);
        assert!(in_alpha > 0.0 && in_alpha < 1.0);
        assert!(
            (out_alpha + in_alpha - 1.0).abs() < 1e-5,
            "a true cross-fade"
        );

        let (end, owes) = paint_at(&mut w, size, None, 400.0);
        assert!(!owes, "a settled lane asks for nothing");
        assert_eq!(end.rects.len(), 1, "only the new bar is left");
        assert_eq!(end.rects[0].2.components[3], 1.0);
    }

    #[test]
    fn reduce_motion_snaps_the_cross_fade() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut counter = 0u64;
        let prev = view("account", TabsVariant::Line);
        let mut w = View::<Picked>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view("password", TabsVariant::Line);
        View::<Picked>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        let size = layout(&mut w, Some(&theme));
        let (rec, owes) = paint_at(&mut w, size, Some(&theme), 0.0);
        assert!(!owes);
        assert_eq!(rec.rects.len(), 1, "no outgoing bar to fade");
        assert_eq!(rec.rects[0].2.components[3], 1.0);
    }

    #[test]
    fn a_disabled_trigger_dims_its_label() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let rec = paint(&mut w, size, None);
        let inactive = rec.glyph_inks[1].components[3];
        let disabled = rec.glyph_inks[2].components[3];
        assert_eq!(disabled, inactive * style::DISABLED_OPACITY);
    }

    // ---- Interaction ------------------------------------------------------

    #[test]
    fn up_inside_a_trigger_reports_its_value_without_self_mutating() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        let hit = trigger_center(&w, 1);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, hit.x, hit.y),
        );
        assert_eq!(w.captured, Some(1));
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, hit.x, hit.y),
        );
        assert_eq!(state.last.as_deref(), Some("password"));
        assert_eq!(w.value, "account", "the app owns `value`");
        assert_eq!(w.captured, None);
    }

    #[test]
    fn a_release_off_the_armed_trigger_or_a_cancel_never_fires() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        let (first, second) = (trigger_center(&w, 0), trigger_center(&w, 1));
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, first.x, first.y),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Up, second.x, second.y),
        );
        assert_eq!(state.count, 0);

        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, first.x, first.y),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Cancel, first.x, first.y),
        );
        assert_eq!(state.count, 0);
        assert_eq!(w.captured, None);
    }

    #[test]
    fn a_disabled_trigger_and_the_area_below_the_strip_take_no_press() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        let third = trigger_center(&w, 2);
        dispatch(
            &mut w,
            &mut state,
            size,
            &pointer(PointerPhase::Down, third.x, third.y),
        );
        assert_eq!(w.captured, None, "the third tab is disabled");

        // Below the strip is the panel's business, not a trigger's.
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                size,
                &pointer(
                    PointerPhase::Down,
                    10.0,
                    TABS_LIST_HEIGHT + TABS_ROOT_GAP + 4.0
                )
            ),
            EventResult::Ignored
        );
        assert_eq!(state.count, 0);
    }

    #[test]
    fn arrow_keys_move_the_focus_and_activate_as_they_go() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        assert_eq!(w.focused_tab, 0);
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.focused_tab, 1);
        assert_eq!(state.last.as_deref(), Some("password"));
        // Wraps past the disabled third tab.
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowRight)),
        );
        assert_eq!(w.focused_tab, 0);
        assert_eq!(state.last.as_deref(), Some("account"));
        dispatch(
            &mut w,
            &mut state,
            size,
            &key(Key::Named(NamedKey::ArrowLeft)),
        );
        assert_eq!(w.focused_tab, 1);
        assert_eq!(w.value, "account", "no key ever wrote `value`");
    }

    #[test]
    fn space_activates_the_focused_tab_and_an_unrelated_key_is_ignored() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        w.focused_tab = 1;
        dispatch(&mut w, &mut state, size, &key(Key::Character(" ".into())));
        assert_eq!(state.last.as_deref(), Some("password"));
        assert_eq!(
            dispatch(&mut w, &mut state, size, &key(Key::Named(NamedKey::Tab))),
            EventResult::Ignored
        );
        assert_eq!(state.count, 1);
    }

    #[test]
    fn the_hover_latch_tracks_the_trigger_under_the_pointer() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        let second = trigger_center(&w, 1);
        let third = trigger_center(&w, 2);

        let state_any: &mut dyn Any = &mut state;
        let mut enter = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut enter, &pointer(PointerPhase::Move, second.x, second.y));
        assert_eq!(w.hovered, Some(1));
        assert!(enter.needs_redraw());

        let state_any: &mut dyn Any = &mut state;
        let mut settled = EventCtx::new(state_any, Point::ZERO, size);
        w.event(
            &mut settled,
            &pointer(PointerPhase::Move, second.x, second.y),
        );
        assert!(
            !settled.needs_redraw(),
            "an unchanged latch asks for nothing"
        );

        let state_any: &mut dyn Any = &mut state;
        let mut over_disabled = EventCtx::new(state_any, Point::ZERO, size);
        w.event(
            &mut over_disabled,
            &pointer(PointerPhase::Move, third.x, third.y),
        );
        assert_eq!(w.hovered, None, "a disabled trigger takes no hover");
    }

    #[test]
    fn paint_drops_a_stale_hover_latch() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        w.hovered = Some(1);
        let _ = paint(&mut w, size, None);
        assert_eq!(w.hovered, None);
    }

    #[test]
    fn a_broadcast_reaches_every_panel_and_is_never_consumed() {
        let (mut w, size) = laid_out("account", TabsVariant::Default, None);
        let mut state = Picked::default();
        let result = dispatch(&mut w, &mut state, size, &InputEvent::Housekeeping);
        assert_eq!(result, EventResult::Ignored);
    }

    #[test]
    fn rebuild_adopts_the_confirmed_value_and_moves_the_focus_to_it() {
        let mut counter = 0u64;
        let prev = view("account", TabsVariant::Default);
        let mut w = View::<Picked>::build(&prev, &mut BuildCtx::new(&mut counter));
        let next = view("password", TabsVariant::Default);
        let flags = View::<Picked>::rebuild(&next, &prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.value, "password");
        assert_eq!(w.focused_tab, 1);
        assert_eq!(w.fade_from, Some(0), "the outgoing bar is recorded");
        assert!(flags.needs_layout(), "a different panel needs measuring");
    }

    #[test]
    fn visit_children_publishes_every_panel_pod() {
        let (w, _) = laid_out("account", TabsVariant::Default, None);
        let mut seen = 0usize;
        Widget::visit_children(&w, &mut |_pod| seen += 1);
        assert_eq!(seen, 3, "hidden panels are still owned pods");
    }

    // ---- Root-driven: focus ring, cursor, semantics ------------------------

    struct Harness {
        root: frust_core::RenderRoot<Picked, TabsView<Picked>>,
        state: Picked,
        tcx: TextContext,
    }

    impl Harness {
        fn new(variant: TabsVariant) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Picked::default(),
                tcx: TextContext::new(),
            };
            h.root.set_theme(Box::new(crate::theme()));
            let mut logic = move |_s: &mut Picked| view("account", variant);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root
                .layout_with_text(Size::new(400.0, 300.0), &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(&mut self, event: &InputEvent) -> EventOutcome {
            self.root.event(&mut self.state, event)
        }

        fn paint(&mut self) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, FrameTime::ZERO);
            rec
        }

        fn semantics(&self) -> SemanticsUpdate {
            self.root.semantics()
        }
    }

    #[test]
    fn a_press_focuses_the_trigger_and_paints_its_ring() {
        let mut h = Harness::new(TabsVariant::Default);
        assert!(h.paint().strokes.is_empty(), "no ring at rest");
        h.dispatch(&pointer(PointerPhase::Down, 20.0, TABS_LIST_HEIGHT / 2.0));
        assert!(h.root.is_focus_active());
        let rec = h.paint();
        assert_eq!(rec.strokes.len(), 1, "the focus ring");
        let (bbox, width, color) = rec.strokes[0];
        assert_eq!(width, style::FOCUS_RING_WIDTH);
        assert_eq!(color.components[3], style::FOCUS_RING_OPACITY);
        assert!(
            bbox.y0 < TABS_LIST_PADDING,
            "painted outside the trigger's box"
        );
    }

    #[test]
    fn hovering_a_trigger_repaints_it_in_full_ink_and_resolves_the_cursor() {
        let mut h = Harness::new(TabsVariant::Default);
        let resting = h.paint();
        assert_eq!(
            resting.glyph_inks[1].components[3], TABS_INACTIVE_INK_ALPHA,
            "the second tab starts dimmed"
        );

        // The second trigger sits past the first; the first is at least ~50px wide
        // for this label, so probe well into the strip.
        let gain = h.dispatch(&pointer(PointerPhase::Move, 90.0, TABS_LIST_HEIGHT / 2.0));
        assert!(gain.needs_redraw, "entering a trigger repaints");
        let hovered = h.paint();
        assert_eq!(
            hovered.glyph_inks[1],
            crate::theme().scheme().on_surface,
            "`hover:text-foreground`"
        );
        assert_eq!(h.root.cursor(), style::ACTIVE_CURSOR);

        // Below the strip nothing asks for a shape.
        h.dispatch(&pointer(
            PointerPhase::Move,
            90.0,
            TABS_LIST_HEIGHT + TABS_ROOT_GAP + 4.0,
        ));
        assert_eq!(h.root.cursor(), CursorIcon::Default);
    }

    #[test]
    fn semantics_yields_a_tablist_of_tabs_plus_the_active_panel_only() {
        let h = Harness::new(TabsVariant::Default);
        let update = h.semantics();
        let list = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::TabList)
            .expect("a Role::TabList node");
        let tabs: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Tab)
            .collect();
        assert_eq!(tabs.len(), 3);
        assert_eq!(list.1.children().len(), 3);
        assert_eq!(tabs[0].1.label(), Some("Account"));
        assert_eq!(tabs[0].1.is_selected(), Some(true));
        assert_eq!(tabs[1].1.is_selected(), Some(false));
        assert!(tabs[2].1.is_disabled());

        // Exactly one panel's content is published — the active one's. A
        // `frust::text` leaf carries its run in the node's `value`.
        let panels: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Label)
            .filter_map(|(_, n)| n.value())
            .collect();
        assert_eq!(panels, vec!["account panel"]);
    }
}
