//! Ports beUI's `dock` component.
//!
//! **Source:** `components/motion/dock.tsx` of the beUI monorepo, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! `Dock`, `DockItem` and `DockSeparator` collapse into one widget: the bar's
//! metrics, its shared active pill and (here) the magnification neighbourhood
//! are all properties of the strip, not of one icon.
//!
//! | upstream | here |
//! |---|---|
//! | bar `items-end gap-1.5 rounded-2xl px-2 py-1` | [`DOCK_GAP`], [`DOCK_PADDING_X`]/[`DOCK_PADDING_Y`], [`style::RADIUS_2XL`] |
//! | bar `border-border bg-card/80` | the hairline plus [`DOCK_BAR_ALPHA`] of `card` |
//! | item `size = 44` (the `size` prop's default) | [`DOCK_ITEM_SIZE`], overridable with [`DockView::size`] |
//! | active pill `absolute inset-0.5 -z-10 rounded-xl bg-primary/5` | [`DOCK_PILL_INSET`], [`style::RADIUS_XL`], [`DOCK_PILL_ALPHA`] |
//! | pill `layoutId` + `SPRING_LAYOUT` | the pill's rect springs between items on [`SPRING_LAYOUT`] |
//! | `DockSeparator` `mx-1 h-6 w-px bg-border` | [`dock_separator`], [`DOCK_SEPARATOR_HEIGHT`] |
//!
//! # Premise correction: upstream has no magnification
//!
//! `dock.tsx` is a **static** bar. Its only motion is the active pill's shared
//! layout travel; there is no `useSpring`, no pointer read, and no scale
//! anywhere in the file. The macOS-style magnification this module ships is
//! therefore an **addition** specified by the porting card, not a transcription,
//! and it is kept behind [`DockView::magnify`] so upstream's own bar remains
//! reachable (`.magnify(false)`). It is on by default because the card asks for
//! a magnifying dock.
//!
//! # The magnification neighbourhood
//!
//! Two independent factors multiply into one scale per item:
//!
//! 1. **Distance falloff** — [`dock_falloff`], a raised cosine over
//!    [`DOCK_MAGNIFY_REACH`] item slots. It is `1` under the pointer, decreases
//!    monotonically, and is exactly `0` at and beyond the reach, so an item
//!    outside the neighbourhood is untouched rather than nearly-untouched.
//! 2. **A hover envelope** — a [`Presence`] on
//!    [`SPRING_MOUSE`](crate::tokens::motion::SPRING_MOUSE), the catalog's
//!    softest spring and the one its pointer-tracked effects are authored with.
//!    It springs in when the pointer arrives and out when it leaves, so the
//!    whole neighbourhood grows and relaxes rather than snapping on and off.
//!
//! The peak scale is [`DOCK_MAGNIFY_SCALE`], applied about each item's **bottom**
//! centre so icons grow upward out of a bar whose baseline never moves — which
//! is what `items-end` means on the web original. Layout reserves
//! `item_size · (DOCK_MAGNIFY_SCALE − 1)` of headroom above the bar for that
//! growth, so a magnified icon stays inside the widget's own box instead of
//! overflowing into a parent's clip.
//!
//! Magnification is **paint-only**: the bar's layout, the icon pods' boxes and
//! the widget's reported size never change with the pointer, so a cursor sweep
//! costs repaints and not relayouts.
//!
//! # The Down-versus-hover rule, decided here
//!
//! frust ends the hover link on every `Down` (`docs/CODE_STANDARDS.md`'s
//! Interaction Semantics), and [`PointerTracker`] is reconciled against
//! `PaintCtx::is_hovered` each paint. Taken literally that would collapse the
//! whole neighbourhood the instant a dock icon is pressed and re-grow it on
//! release — the icon shrinking out from under the cursor that is pressing it.
//!
//! **This component keeps the magnification alive across a press.** While it
//! holds a press it armed, `paint` skips the hover reconciliation entirely and
//! keeps reading positions from the captured moves, so the neighbourhood tracks
//! the pointer through a drag exactly as it did before the button went down. The
//! reconciliation resumes on the very next paint after the release. The rule is
//! deliberately narrow: only a press *this widget armed* suspends it, so a
//! pointer captured by something else still collapses the dock.
//!
//! **The release is where the rule stops, deliberately.** `EventCtx::claim_hover`
//! records nothing outside an uncaptured `Move` pass, and an `Up` ends whatever
//! hover stood without opening a new one — the framework states outright that a
//! consumer re-claims on the next `Move` rather than expecting its chrome to
//! survive a click. So this widget does not try to re-open the link on release:
//! the neighbourhood relaxes on [`SPRING_MOUSE`](crate::tokens::motion::SPRING_MOUSE)
//! after a click and springs back on the pointer's next move. Latching past the
//! release instead would strand a magnified dock whenever the pointer left onto
//! a sibling, because a widget is told nothing about a hover link opening
//! somewhere else.
//!
//! That relaxation is a real difference from the web original, where `:hover`
//! simply persists across a click, and it is a substrate gap rather than a
//! component one: [`PointerTracker`] additionally resets itself on `Up`/`Cancel`
//! (right for a click-once control, wrong for a hover effect). `motion/pointer.rs`
//! is outside this card's scope, so the finding is recorded in the task summary
//! rather than patched there.
//!
//! # Degradations against the web original
//!
//! - **No `backdrop-blur-xl` and no `shadow-2xl`.** `PaintScene` publishes no
//!   backdrop filter, and beUI authors no shadow token this port could resolve a
//!   `2xl` elevation from, so the bar is a translucent `card` fill with its
//!   hairline and nothing behind it.
//! - **No focus ring.** Upstream's item carries `focus-visible:ring-2 …
//!   ring-offset-2`; this port has no per-item focus target because the whole
//!   bar is one widget, so keyboard focus is not modelled at all — a dock is a
//!   pointer affordance here.
//! - **`DockItem` takes an explicit icon child, never arbitrary link children.**
//!   Upstream's item wraps whatever is passed (often an `<a>` carrying its own
//!   accessible name); here the icon is a view and the accessible name is the
//!   item's own [`DockItem::label`].

use std::rc::Rc;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod, Color,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Rect,
    Role, SemanticsCtx, Size, View, Widget, any, build_child, erase_callback_arg, rebuild_children,
    route_event, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, Theme};

use crate::motion::pointer::PointerTracker;
use crate::motion::{Presence, Ramp};
use crate::press::presses;
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::{SPRING_LAYOUT, SPRING_MOUSE};

/// One dock icon's box, in logical px — upstream's `size` prop default (`44`).
pub const DOCK_ITEM_SIZE: f64 = 44.0;

/// The gap between icons, in logical px (`gap-1.5`).
pub const DOCK_GAP: f64 = 6.0;

/// The bar's horizontal padding, in logical px (`px-2`).
pub const DOCK_PADDING_X: f64 = 8.0;

/// The bar's vertical padding, in logical px (`py-1`).
pub const DOCK_PADDING_Y: f64 = 4.0;

/// The bar fill's alpha (`bg-card/80`).
pub const DOCK_BAR_ALPHA: f32 = 0.80;

/// How far the active pill is inset inside its item, in logical px
/// (`inset-0.5`).
pub const DOCK_PILL_INSET: f64 = 2.0;

/// The active pill's alpha (`bg-primary/5`).
pub const DOCK_PILL_ALPHA: f32 = 0.05;

/// A separator's rule height, in logical px (`h-6`).
pub const DOCK_SEPARATOR_HEIGHT: f64 = 24.0;

/// A separator's margin on each side, in logical px (`mx-1`).
pub const DOCK_SEPARATOR_MARGIN: f64 = 4.0;

/// The peak magnification factor, reached by the icon directly under the
/// pointer. Not upstream — see the [module docs](self).
pub const DOCK_MAGNIFY_SCALE: f64 = 1.6;

/// How far the magnification neighbourhood reaches, in item slots. An icon this
/// many slots from the pointer is exactly unmagnified.
pub const DOCK_MAGNIFY_REACH: f64 = 2.0;

/// The gap between an item's label and the top of its icon, in logical px.
pub const DOCK_LABEL_GAP: f64 = 6.0;

/// The magnification falloff: how strongly an icon `distance` item-slots from
/// the pointer is magnified, in `[0, 1]`.
///
/// A raised cosine over `reach` slots — `1` under the pointer, `0` at and beyond
/// the reach, smooth (zero-derivative) at both ends, so the neighbourhood has no
/// visible seam where it stops. A non-positive `reach` degenerates to "only the
/// icon exactly under the pointer", never to a division by zero.
pub fn dock_falloff(distance: f64, reach: f64) -> f64 {
    if reach <= 0.0 {
        return if distance == 0.0 { 1.0 } else { 0.0 };
    }
    let d = distance.abs();
    if d >= reach {
        return 0.0;
    }
    0.5 * (1.0 + (std::f64::consts::PI * d / reach).cos())
}

/// One dock entry: an icon view, an optional accessible/hover label, and whether
/// it is the active one. [`dock_separator`] builds the rule variant.
pub struct DockItem<State: 'static> {
    icon: AnyView<State>,
    label: Option<String>,
    active: bool,
    disabled: bool,
    separator: bool,
}

/// Create a dock entry rendering `icon`.
pub fn dock_item<State: 'static, V: View<State>>(icon: V) -> DockItem<State> {
    DockItem {
        icon: any(icon),
        label: None,
        active: false,
        disabled: false,
        separator: false,
    }
}

/// Create a dock separator — upstream's `DockSeparator`, a centred hairline
/// between two groups of icons.
///
/// It still carries a (never-painted) child view so the entry list and the pod
/// list stay index-for-index aligned; a separate collection would make every
/// hit-test and every pill lookup translate between two indices.
pub fn dock_separator<State: 'static>() -> DockItem<State> {
    DockItem {
        icon: any(frust::text("")),
        label: None,
        active: false,
        disabled: false,
        separator: true,
    }
}

impl<State: 'static> DockItem<State> {
    /// Name this entry — its accessible name, and the caption shown above it
    /// while the pointer is on it.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Mark this entry active, so the shared pill rests behind it.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Make this entry inert: dimmed and unpressable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A view-held selection callback, erased on build.
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// A declarative beUI dock. See the [module docs](self).
pub struct DockView<State: 'static> {
    items: Vec<DockItem<State>>,
    size: f64,
    magnify: bool,
    on_select: OnSelect<State>,
}

/// Create a dock over `items`, reporting the index of a pressed entry through
/// `on_select`.
pub fn dock<State: 'static, F: Fn(&mut State, usize) + 'static>(
    items: Vec<DockItem<State>>,
    on_select: F,
) -> DockView<State> {
    DockView {
        items,
        size: DOCK_ITEM_SIZE,
        magnify: true,
        on_select: Rc::new(on_select),
    }
}

impl<State: 'static> DockView<State> {
    /// Set each icon's box, in logical px (upstream's `size` prop).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0);
        self
    }

    /// Turn the pointer magnification on or off. Off is upstream's own static
    /// bar — see the [module docs](self)' premise correction.
    pub fn magnify(mut self, magnify: bool) -> Self {
        self.magnify = magnify;
        self
    }
}

/// The resolved dock palette.
struct DockColors {
    /// The bar fill, already at [`DOCK_BAR_ALPHA`] (`bg-card/80`).
    bar: Color,
    /// The bar's hairline and the separator rule (`border-border`).
    border: Color,
    /// The active pill, already at [`DOCK_PILL_ALPHA`] (`bg-primary/5`).
    pill: Color,
    /// The caption ink (`text-foreground`).
    label: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> DockColors {
    let (card, border, primary, foreground) = match theme {
        Some(theme) => {
            let s = theme.scheme();
            (
                s.surface_container,
                s.outline_variant,
                s.primary,
                s.on_surface,
            )
        }
        None => {
            let p = crate::BEUI_LIGHT;
            (p.card, p.border, p.primary, p.foreground)
        }
    };
    DockColors {
        bar: style::with_alpha(card, DOCK_BAR_ALPHA),
        border,
        pill: style::with_alpha(primary, DOCK_PILL_ALPHA),
        label: foreground,
    }
}

/// The caption style: the theme's Geist stack at `text-xs`/`font-medium`.
fn label_style(theme: Option<&Theme>) -> TextStyle {
    let family = theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_small.family.clone()
    });
    TextStyle {
        family,
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_XS as f32, Color::BLACK)
    }
}

/// Linear interpolation between two rects, `t` unclamped.
fn lerp_rect(from: Rect, to: Rect, t: f64) -> Rect {
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    Rect::new(
        lerp(from.x0, to.x0),
        lerp(from.y0, to.y0),
        lerp(from.x1, to.x1),
        lerp(from.y1, to.y1),
    )
}

/// One retained entry: its metadata plus the geometry layout resolved.
struct Entry {
    label: Option<LabelRun>,
    name: Option<String>,
    active: bool,
    disabled: bool,
    separator: bool,
    /// Local x of the entry's left edge (filled at layout).
    x: f64,
    /// The entry's advance width (filled at layout).
    width: f64,
}

impl Entry {
    fn from_item<State: 'static>(item: &DockItem<State>) -> Self {
        Self {
            label: item.label.clone().map(LabelRun::new),
            name: item.label.clone(),
            active: item.active,
            disabled: item.disabled,
            separator: item.separator,
            x: 0.0,
            width: 0.0,
        }
    }
}

impl<State: 'static> View<State> for DockView<State> {
    type Element = DockWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> DockWidget {
        DockWidget {
            entries: self.items.iter().map(Entry::from_item).collect(),
            icons: self
                .items
                .iter()
                .map(|item| build_child(&item.icon, ctx))
                .collect(),
            item_size: self.size,
            magnify: self.magnify,
            tracker: PointerTracker::new(),
            envelope: Presence::new(Ramp::spring(SPRING_MOUSE), Ramp::spring(SPRING_MOUSE)),
            envelope_value: 0.0,
            captured: None,
            pill_from: None,
            pill_shown: None,
            pill_started: None,
            bar_top: 0.0,
            label_band: 0.0,
            on_select: erase_callback_arg(&self.on_select),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut DockWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_select = erase_callback_arg(&self.on_select);
        let mut flags = ChangeFlags::NONE;

        if prev.items.len() != self.items.len() {
            element.entries = self.items.iter().map(Entry::from_item).collect();
            element.captured = None;
            element.pill_from = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            let active_before = element.active_index();
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.name != item.label {
                    entry.label = item.label.clone().map(LabelRun::new);
                    entry.name = item.label.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.active != item.active
                    || entry.disabled != item.disabled
                    || entry.separator != item.separator
                {
                    entry.active = item.active;
                    entry.disabled = item.disabled;
                    entry.separator = item.separator;
                    flags |= ChangeFlags::PAINT;
                }
            }
            if active_before != element.active_index() {
                // Launch the pill from where it is displaying, not from the item
                // it nominally belonged to.
                element.pill_from = element.pill_shown.or_else(|| element.pill_target());
                element.pill_started = None;
                flags |= ChangeFlags::PAINT;
            }
        }

        if prev.size != self.size || prev.magnify != self.magnify {
            element.item_size = self.size;
            element.magnify = self.magnify;
            element.pill_from = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.icons,
            ctx,
            |item: &DockItem<State>| &item.icon,
            |_| None::<ChildKey>,
        );
        flags
    }

    fn teardown(&self, element: &mut DockWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.icons.iter_mut()) {
            teardown_child(&item.icon, pod, ctx);
        }
    }
}

/// The retained widget for a [`DockView`].
pub struct DockWidget {
    entries: Vec<Entry>,
    /// One icon pod per entry, separators included (never painted for those).
    icons: Vec<ChildPod>,
    item_size: f64,
    magnify: bool,
    /// Where the pointer is inside the bar, and whether it is here at all.
    tracker: PointerTracker,
    /// The magnification envelope: springs in on hover, out on leave.
    envelope: Presence,
    /// The envelope's last painted value.
    envelope_value: f64,
    /// The entry a `Down` armed, cleared on `Up`/`Cancel`.
    captured: Option<usize>,
    /// The rect the pill's current travel started from.
    pill_from: Option<Rect>,
    /// The rect the pill painted last frame — the retarget origin.
    pill_shown: Option<Rect>,
    /// The frame the current pill travel was first painted at.
    pill_started: Option<FrameTime>,
    /// Local y of the bar band's top edge (filled at layout).
    bar_top: f64,
    /// The caption band's height above the magnification headroom.
    label_band: f64,
    on_select: frust::authoring::ErasedArgCallback<usize>,
}

impl DockWidget {
    /// The active entry's index, if any.
    fn active_index(&self) -> Option<usize> {
        self.entries.iter().position(|e| e.active && !e.separator)
    }

    /// Whether entry `index` can be pressed.
    fn enabled(&self, index: usize) -> bool {
        self.entries
            .get(index)
            .is_some_and(|e| !e.disabled && !e.separator)
    }

    /// The magnification headroom reserved above the bar.
    fn headroom(&self) -> f64 {
        if self.magnify {
            self.item_size * (DOCK_MAGNIFY_SCALE - 1.0)
        } else {
            0.0
        }
    }

    /// The bar band's own height.
    fn bar_height(&self) -> f64 {
        self.item_size + DOCK_PADDING_Y * 2.0
    }

    /// The distance between two adjacent icon centres — the unit
    /// [`dock_falloff`] measures in.
    fn slot_pitch(&self) -> f64 {
        self.item_size + DOCK_GAP
    }

    /// Entry `index`'s resting icon box (bar-relative, unmagnified).
    fn item_rect(&self, index: usize) -> Option<Rect> {
        let entry = self.entries.get(index)?;
        if entry.separator {
            return None;
        }
        Some(Rect::from_origin_size(
            Point::new(entry.x, self.bar_top + DOCK_PADDING_Y),
            Size::new(self.item_size, self.item_size),
        ))
    }

    /// The active pill's resting rect.
    fn pill_target(&self) -> Option<Rect> {
        let rect = self.item_rect(self.active_index()?)?;
        Some(rect.inset(-DOCK_PILL_INSET))
    }

    /// The bar's own width.
    fn bar_width(&self) -> f64 {
        let content: f64 = self.entries.iter().map(|e| e.width).sum();
        let gaps = DOCK_GAP * self.entries.len().saturating_sub(1) as f64;
        content + gaps + DOCK_PADDING_X * 2.0
    }

    /// The entry under a widget-local `pos`, if any (the bar band only).
    fn hit_item(&self, pos: Point) -> Option<usize> {
        (0..self.entries.len())
            .find(|index| self.item_rect(*index).is_some_and(|r| r.contains(pos)))
    }

    /// Entry `index`'s magnification factor at the tracked pointer position.
    ///
    /// `1.0` with magnification off, with the pointer away, or for a separator;
    /// otherwise [`dock_falloff`] over the distance in slots, scaled by the
    /// hover envelope.
    fn magnification(&self, index: usize) -> f64 {
        if !self.magnify || self.envelope_value <= 0.0 {
            return 1.0;
        }
        let Some(rect) = self.item_rect(index) else {
            return 1.0;
        };
        let distance = (rect.center().x - self.tracker.position().x).abs() / self.slot_pitch();
        1.0 + (DOCK_MAGNIFY_SCALE - 1.0)
            * dock_falloff(distance, DOCK_MAGNIFY_REACH)
            * self.envelope_value
    }

    /// Advance the pill to `now`, returning the rect to paint and whether the
    /// travel is still running.
    fn advance_pill(&mut self, now: FrameTime, reduce_motion: bool) -> (Option<Rect>, bool) {
        let Some(target) = self.pill_target() else {
            self.pill_shown = None;
            return (None, false);
        };
        let Some(from) = self.pill_from.filter(|_| !reduce_motion) else {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        };
        let ramp = Ramp::spring(SPRING_LAYOUT);
        let started = *self.pill_started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if ramp.is_settled(elapsed) {
            self.pill_from = None;
            self.pill_started = None;
            self.pill_shown = Some(target);
            return (Some(target), false);
        }
        let shown = lerp_rect(from, target, ramp.progress(elapsed));
        self.pill_shown = Some(shown);
        (Some(shown), true)
    }
}

impl Widget for DockWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let caption = label_style(theme);

        // The caption band is reserved whenever any entry can show one, so the
        // bar does not shift up and down as the pointer crosses it.
        let mut caption_height: f64 = 0.0;
        for entry in &mut self.entries {
            if let Some(run) = &mut entry.label {
                caption_height = caption_height.max(run.layout(ctx, &caption).height);
            }
        }
        self.label_band = if caption_height > 0.0 {
            caption_height + DOCK_LABEL_GAP
        } else {
            0.0
        };
        self.bar_top = self.label_band + self.headroom();

        let mut x = DOCK_PADDING_X;
        for entry in &mut self.entries {
            entry.width = if entry.separator {
                DOCK_SEPARATOR_MARGIN * 2.0 + style::BORDER_WIDTH
            } else {
                self.item_size
            };
            entry.x = x;
            x += entry.width + DOCK_GAP;
        }

        // Icons are laid out at their resting box; magnification is a paint-time
        // transform and never re-measures them.
        let item_bc = BoxConstraints::new(Size::ZERO, Size::new(self.item_size, self.item_size));
        for index in 0..self.entries.len() {
            let rect = self.item_rect(index);
            let Some(pod) = self.icons.get_mut(index) else {
                continue;
            };
            let size = pod.layout_child(ctx, &item_bc);
            if let Some(rect) = rect {
                pod.set_origin(Point::new(
                    rect.x0 + (self.item_size - size.width) / 2.0,
                    rect.y0 + (self.item_size - size.height) / 2.0,
                ));
            }
        }

        bc.constrain(Size::new(
            self.bar_width(),
            self.bar_top + self.bar_height(),
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // The Down-versus-hover rule: a press this widget armed suspends the
        // hover reconciliation, so the neighbourhood survives the press instead
        // of collapsing under the cursor pressing it (see the module docs).
        if self.captured.is_none() {
            self.tracker.sync_hovered(ctx.is_hovered());
        }
        self.tracker.set_size(ctx.size());

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();

        if reduce_motion {
            self.envelope = self.envelope.collapsed();
        }
        self.envelope
            .set_open(self.tracker.hovered() && self.magnify);
        self.envelope_value = self.envelope.advance(ctx.frame_time()).clamp(0.0, 1.0);
        let envelope_running = self.envelope.is_animating();

        let (pill, pill_running) = self.advance_pill(ctx.frame_time(), reduce_motion);

        // The bar band: a translucent `card` fill with its hairline.
        let bar_origin = Point::new(origin.x, origin.y + self.bar_top);
        let bar_size = Size::new(self.bar_width(), self.bar_height());
        let radius = style::resolve_radius(style::RADIUS_2XL, bar_size.width, bar_size.height);
        scene.fill_rounded_rect(bar_origin, bar_size, radius, colors.bar);
        scene.stroke_path(
            bar_origin,
            &frust::authoring::Shape::to_path(
                &frust::authoring::RoundedRect::from_rect(
                    Rect::from_origin_size(Point::ORIGIN, bar_size)
                        .inset(-style::BORDER_WIDTH / 2.0),
                    radius,
                ),
                style::PATH_TOLERANCE,
            ),
            style::BORDER_WIDTH,
            &Brush::Solid(colors.border),
        );

        // The active pill, under the icons.
        if let Some(rect) = pill {
            scene.fill_rounded_rect(
                Point::new(origin.x + rect.x0, origin.y + rect.y0),
                Size::new(rect.width(), rect.height()),
                style::RADIUS_XL,
                colors.pill,
            );
        }

        // Separator rules, centred in the bar band.
        for entry in &self.entries {
            if !entry.separator {
                continue;
            }
            scene.fill_rect(
                Point::new(
                    origin.x + entry.x + DOCK_SEPARATOR_MARGIN,
                    origin.y + self.bar_top + (self.bar_height() - DOCK_SEPARATOR_HEIGHT) / 2.0,
                ),
                Size::new(style::BORDER_WIDTH, DOCK_SEPARATOR_HEIGHT),
                colors.border,
            );
        }

        // Icons, each scaled about its own bottom centre so the bar's baseline
        // never moves (`items-end`).
        for index in 0..self.entries.len() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            let dimmed = !self.enabled(index);
            let scale = self.magnification(index);
            let anchor = Point::new(origin.x + rect.center().x, origin.y + rect.y1);
            let magnified = scale != 1.0;
            if magnified {
                scene.push_transform(
                    Affine::translate(anchor.to_vec2())
                        * Affine::scale(scale)
                        * Affine::translate(-anchor.to_vec2()),
                );
            }
            if dimmed {
                scene.push_layer(
                    Point::new(origin.x + rect.x0, origin.y + rect.y0),
                    Size::new(rect.width(), rect.height()),
                    style::DISABLED_OPACITY,
                );
            }
            if let Some(pod) = self.icons.get_mut(index) {
                pod.paint_child(ctx, scene);
            }
            if dimmed {
                scene.pop_layer();
            }
            if magnified {
                scene.pop_transform();
            }
        }

        // The caption for the entry under the pointer, centred over its icon in
        // the reserved band.
        if self.label_band > 0.0
            && let Some(index) = self
                .tracker
                .hovered()
                .then(|| self.hit_item(self.tracker.position()))
                .flatten()
            && let Some(entry) = self.entries.get(index)
            && let (Some(run), Some(rect)) = (&entry.label, self.item_rect(index))
        {
            let label = run.size();
            run.paint(
                Point::new(
                    origin.x + rect.center().x - label.width / 2.0,
                    origin.y + (self.label_band - DOCK_LABEL_GAP - label.height).max(0.0),
                ),
                colors.label,
                scene,
            );
        }

        // Magnification and the pill both move within an already-measured box,
        // so a bare frame request covers both.
        if envelope_running || pill_running {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.icons {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }

        // Icons are decorative children; they are routed first so an interactive
        // one still wins, and the bar's own handling is the fallback.
        if route_event(&mut self.icons, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }

        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        match p.phase {
            PointerPhase::Down => {
                if self.tracker.on_pointer(p, size) {
                    ctx.request_redraw();
                }
                if !presses(p) {
                    return EventResult::Ignored;
                }
                let Some(index) = self.hit_item(p.position).filter(|i| self.enabled(*i)) else {
                    return EventResult::Ignored;
                };
                self.captured = Some(index);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if self.tracker.on_pointer(p, size) {
                    ctx.request_redraw();
                }
                if self.captured.is_some() {
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                    return EventResult::Handled;
                }
                if self.tracker.hovered() {
                    ctx.claim_hover();
                    if self.hit_item(p.position).is_some_and(|i| self.enabled(i)) {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    }
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                let armed = self.captured.take();
                // The tracker drops itself here and the framework's hover link
                // ends with it; the neighbourhood relaxes until the pointer's
                // next move re-claims (see the module docs).
                self.tracker.on_pointer(p, size);
                ctx.request_redraw();
                let Some(armed) = armed else {
                    return EventResult::Ignored;
                };
                if self.hit_item(p.position) == Some(armed) {
                    (self.on_select)(ctx, armed);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                self.tracker.on_pointer(p, size);
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
            Role::Toolbar,
            |_| {},
            |ctx| {
                for entry in self.entries.iter().filter(|e| !e.separator) {
                    ctx.push_node(Role::Button, |node| {
                        if let Some(name) = &entry.name {
                            node.set_label(name.as_str());
                        }
                        node.set_selected(entry.active);
                        if entry.disabled {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
            },
        );
    }

    visit_children!(icons);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{BezPath, PointerButton, PointerEvent};
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rects: Vec<(Point, Size, Color)>,
        rrects: Vec<(Point, Size, f64, Color)>,
        transforms: Vec<Affine>,
        inks: Vec<Color>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, o: Point, s: Size, c: Color) {
            self.rects.push((o, s, c));
        }
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, r: f64, c: Color) {
            self.rrects.push((o, s, r, c));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
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
        last: Option<usize>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items(active: usize) -> Vec<DockItem<Picked>> {
        vec![
            dock_item(text("F")).label("Finder").active(active == 0),
            dock_item(text("M")).label("Mail").active(active == 1),
            dock_separator(),
            dock_item(text("T")).label("Trash").active(active == 3),
            dock_item(text("X")).label("Gone").disabled(true),
        ]
    }

    fn view(active: usize, magnify: bool) -> DockView<Picked> {
        dock::<Picked, _>(items(active), |s: &mut Picked, i: usize| {
            s.last = Some(i);
            s.count += 1;
        })
        .magnify(magnify)
    }

    fn build(active: usize, magnify: bool) -> DockWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(active, magnify), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut DockWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(800.0, 300.0)),
        )
    }

    fn laid_out(active: usize, magnify: bool) -> (DockWidget, Size) {
        let mut w = build(active, magnify);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut DockWidget,
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

    fn pointer(phase: PointerPhase, position: Point) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position,
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut DockWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// The falloff is a proper neighbourhood: full under the pointer, strictly
    /// decreasing outward, and exactly zero at and past the reach.
    #[test]
    fn the_falloff_peaks_under_the_pointer_and_dies_at_the_reach() {
        assert_eq!(dock_falloff(0.0, DOCK_MAGNIFY_REACH), 1.0);
        assert_eq!(dock_falloff(DOCK_MAGNIFY_REACH, DOCK_MAGNIFY_REACH), 0.0);
        assert_eq!(dock_falloff(9.0, DOCK_MAGNIFY_REACH), 0.0);

        let mut previous = f64::INFINITY;
        for step in 0..=40 {
            let d = DOCK_MAGNIFY_REACH * step as f64 / 40.0;
            let value = dock_falloff(d, DOCK_MAGNIFY_REACH);
            assert!((0.0..=1.0).contains(&value), "{value} left [0, 1] at {d}");
            assert!(value < previous || step == 0, "not decreasing at {d}");
            previous = value;
        }
        // Symmetric: a neighbour to the left magnifies exactly like one to the
        // right.
        assert_eq!(
            dock_falloff(-0.7, DOCK_MAGNIFY_REACH),
            dock_falloff(0.7, DOCK_MAGNIFY_REACH)
        );
        // A degenerate reach is a point neighbourhood, not a division by zero.
        assert_eq!(dock_falloff(0.0, 0.0), 1.0);
        assert_eq!(dock_falloff(0.1, 0.0), 0.0);
    }

    /// The bar geometry a harness test computes its pointer targets from: the
    /// fixture's five entries advance by their own widths, so an icon centre is
    /// derivable without reaching into the widget.
    fn item_centre(index: usize, bar_top: f64) -> Point {
        let widths = [
            DOCK_ITEM_SIZE,
            DOCK_ITEM_SIZE,
            DOCK_SEPARATOR_MARGIN * 2.0 + style::BORDER_WIDTH,
            DOCK_ITEM_SIZE,
            DOCK_ITEM_SIZE,
        ];
        let x = DOCK_PADDING_X + widths[..index].iter().sum::<f64>() + DOCK_GAP * index as f64;
        Point::new(
            x + widths[index] / 2.0,
            bar_top + DOCK_PADDING_Y + DOCK_ITEM_SIZE / 2.0,
        )
    }

    /// A real `RenderRoot`, the only way to get an actual hover link on the
    /// widget's path: `PaintCtx::for_test` reports none, so a direct paint would
    /// self-correct the tracker away before magnification could be read.
    struct Harness {
        root: frust_core::RenderRoot<Picked, DockView<Picked>>,
        state: Picked,
        tcx: TextContext,
    }

    impl Harness {
        fn new(active: usize, magnify: bool, theme: Option<Theme>) -> Self {
            let mut h = Harness {
                root: frust_core::RenderRoot::new(),
                state: Picked::default(),
                tcx: TextContext::new(),
            };
            if let Some(theme) = theme {
                h.root.set_theme(Box::new(theme));
            }
            let mut logic = move |_s: &mut Picked| view(active, magnify);
            h.root.rebuild(&mut logic, &mut h.state);
            h.root
                .layout_with_text(Size::new(800.0, 300.0), &mut h.tcx as &mut dyn Any);
            h
        }

        fn dispatch(&mut self, event: &InputEvent) {
            self.root.event(&mut self.state, event);
        }

        fn paint(&mut self, ms: f64) -> Recorder {
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(ms));
            rec
        }

        /// Paint past the envelope spring's settle time and return the last
        /// frame's recording.
        fn settled_paint(&mut self, from_ms: f64) -> Recorder {
            self.paint(from_ms);
            self.paint(from_ms + 4_000.0)
        }
    }

    /// The bar band's top edge, read back off the painted bar rect.
    fn bar_top_of(rec: &Recorder) -> f64 {
        rec.rrects
            .iter()
            .find(|(_, s, _, _)| s.height == DOCK_ITEM_SIZE + DOCK_PADDING_Y * 2.0)
            .expect("the bar band is painted")
            .0
            .y
    }

    /// The per-icon scale factors a recording carries, in paint order.
    fn scales(rec: &Recorder) -> Vec<f64> {
        rec.transforms.iter().map(|t| t.as_coeffs()[0]).collect()
    }

    /// With the pointer on an icon, that icon is at the peak scale, its
    /// neighbour is magnified less, and an icon past the reach is untouched
    /// (it contributes no transform at all).
    #[test]
    fn magnification_falls_off_smoothly_across_the_bar() {
        let mut h = Harness::new(0, true, None);
        let bar_top = bar_top_of(&h.paint(0.0));
        h.dispatch(&pointer(PointerPhase::Move, item_centre(0, bar_top)));
        let rec = h.settled_paint(0.0);

        let scales = scales(&rec);
        assert_eq!(
            scales.len(),
            2,
            "only the pointer's own neighbourhood scales: {scales:?}"
        );
        assert!(
            (scales[0] - DOCK_MAGNIFY_SCALE).abs() < 1e-6,
            "the icon under the pointer is at the peak: {scales:?}"
        );
        assert!(
            scales[1] > 1.0 && scales[1] < scales[0],
            "the neighbour is magnified less: {scales:?}"
        );
    }

    /// Leaving resets the whole neighbourhood: the envelope springs back and
    /// every icon returns to its resting scale.
    #[test]
    fn the_neighbourhood_resets_when_the_pointer_leaves() {
        let mut h = Harness::new(0, true, None);
        let bar_top = bar_top_of(&h.paint(0.0));
        h.dispatch(&pointer(PointerPhase::Move, item_centre(0, bar_top)));
        assert!(!scales(&h.settled_paint(0.0)).is_empty());

        // A move onto a point outside the bar is the leave frust never sends as
        // an event of its own.
        h.dispatch(&pointer(PointerPhase::Move, Point::new(700.0, 280.0)));
        let rec = h.settled_paint(5_000.0);
        assert!(
            scales(&rec).is_empty(),
            "an icon is still magnified: {:?}",
            scales(&rec)
        );
    }

    /// The decided Down-versus-hover rule: the neighbourhood survives a press
    /// this widget armed, relaxes at the release the framework ends hover on,
    /// and returns on the pointer's next move.
    #[test]
    fn magnification_survives_a_press_and_returns_after_its_release() {
        let mut h = Harness::new(0, true, None);
        let bar_top = bar_top_of(&h.paint(0.0));
        let at = item_centre(1, bar_top);
        h.dispatch(&pointer(PointerPhase::Move, at));
        let hovered = scales(&h.settled_paint(0.0));
        assert!(!hovered.is_empty());

        // The framework ends the hover link on `Down`; the rule keeps the
        // neighbourhood anyway.
        h.dispatch(&pointer(PointerPhase::Down, at));
        assert_eq!(
            scales(&h.paint(5_000.0)),
            hovered,
            "the press collapsed the neighbourhood"
        );

        h.dispatch(&pointer(PointerPhase::Up, at));
        assert_eq!(h.state.last, Some(1), "the press still reports");

        // The rule stops at the release: the framework ends the hover link
        // there, so the neighbourhood relaxes...
        assert!(
            scales(&h.settled_paint(5_001.0)).is_empty(),
            "the release must not latch the neighbourhood open"
        );
        // ...and the pointer's next move brings it straight back.
        h.dispatch(&pointer(PointerPhase::Move, at));
        assert_eq!(scales(&h.settled_paint(10_000.0)), hovered);
    }

    /// Only the entry under the pointer shows its caption.
    #[test]
    fn only_the_hovered_entry_shows_its_caption() {
        let mut h = Harness::new(0, true, None);
        let resting = h.paint(0.0);
        let bar_top = bar_top_of(&resting);
        // The fixture's icons are text leaves, so the baseline is "whatever the
        // icons ink"; the caption is the one run on top of that.
        let icons = resting.inks.len();

        h.dispatch(&pointer(PointerPhase::Move, item_centre(1, bar_top)));
        let rec = h.paint(1.0);
        assert_eq!(rec.inks.len(), icons + 1, "exactly one caption is inked");
        assert!(
            rec.inks.contains(&crate::theme().scheme().on_surface),
            "the caption takes the `foreground` ink"
        );
    }

    /// `reduce_motion` collapses the envelope: the magnification still applies
    /// (it reads a position, it is not itself an animation) but it arrives
    /// without a spring.
    #[test]
    fn reduce_motion_collapses_the_hover_envelope() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let mut h = Harness::new(0, true, Some(theme));
        let bar_top = bar_top_of(&h.paint(0.0));
        h.dispatch(&pointer(PointerPhase::Move, item_centre(0, bar_top)));

        // One frame, not a settle loop: a collapsed envelope is already home.
        let scales = scales(&h.paint(1.0));
        assert!(
            (scales[0] - DOCK_MAGNIFY_SCALE).abs() < 1e-6,
            "the collapse should land on the peak immediately: {scales:?}"
        );
    }

    /// With magnification off — upstream's own bar — nothing scales and no
    /// headroom is reserved.
    #[test]
    fn the_upstream_bar_has_no_magnification_and_no_headroom() {
        let (mut w, size) = laid_out(0, false);
        let mut state = Picked::default();
        assert_eq!(w.headroom(), 0.0);
        let centre = w.item_rect(0).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Move, centre),
            &mut state,
        );
        let (rec, needs_frame) = paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.magnification(0), 1.0);
        assert!(rec.transforms.is_empty(), "nothing is scaled");
        assert!(!needs_frame, "and nothing is animating");
    }

    /// The bar reserves its headroom above the band so a magnified icon stays
    /// inside the widget's own box.
    #[test]
    fn the_magnifying_bar_reserves_headroom_for_the_peak_scale() {
        let (w, size) = laid_out(0, true);
        assert_eq!(w.headroom(), DOCK_ITEM_SIZE * (DOCK_MAGNIFY_SCALE - 1.0));
        assert_eq!(size.height, w.bar_top + w.bar_height());
        let top_icon = w.item_rect(0).unwrap();
        let magnified_top = top_icon.y1 - DOCK_ITEM_SIZE * DOCK_MAGNIFY_SCALE;
        assert!(
            magnified_top >= w.label_band - 1e-9,
            "a peak-scaled icon escapes the reserved band: {magnified_top}"
        );
    }

    /// A separator takes its own advance, paints a rule and is never a press
    /// target.
    #[test]
    fn a_separator_takes_a_slot_but_never_a_press() {
        let (mut w, size) = laid_out(0, true);
        let mut state = Picked::default();
        assert!(w.item_rect(2).is_none(), "a separator has no icon box");
        assert!(!w.enabled(2));
        assert_eq!(
            w.entries[2].width,
            DOCK_SEPARATOR_MARGIN * 2.0 + style::BORDER_WIDTH
        );

        let (rec, _) = paint_at(&mut w, size, None, 0.0);
        assert!(
            rec.rects.iter().any(
                |(_, s, c)| s.height == DOCK_SEPARATOR_HEIGHT && *c == crate::BEUI_LIGHT.border
            ),
            "the rule is painted"
        );

        let at = Point::new(w.entries[2].x + 1.0, w.bar_top + w.bar_height() / 2.0);
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.count, 0);
    }

    /// The active pill rests behind the active item and springs onto the new one
    /// when the app moves it — upstream's `layoutId` travel on `SPRING_LAYOUT`.
    #[test]
    fn the_active_pill_springs_between_items() {
        let (mut w, size) = laid_out(0, true);
        paint_at(&mut w, size, None, 0.0);
        let from = w.item_rect(0).unwrap().inset(-DOCK_PILL_INSET);
        assert_eq!(w.pill_shown, Some(from));

        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(&view(3, true), &view(0, true), &mut w, &mut ctx);
        layout(&mut w);

        let (_, needs_frame) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_frame, "a travelling pill owes frames");
        paint_at(&mut w, size, None, 140.0);
        let mid = w.pill_shown.unwrap();
        let to = w.item_rect(3).unwrap().inset(-DOCK_PILL_INSET);
        assert!(mid.x0 > from.x0 && mid.x0 < to.x0, "mid-flight {mid:?}");

        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(w.pill_shown, Some(to));
    }

    /// A press reports the pressed index; a disabled entry reports nothing.
    #[test]
    fn a_press_reports_its_index_and_a_disabled_entry_does_not() {
        let (mut w, size) = laid_out(0, true);
        let mut state = Picked::default();
        let at = w.item_rect(3).unwrap().center();
        dispatch(&mut w, size, &pointer(PointerPhase::Down, at), &mut state);
        dispatch(&mut w, size, &pointer(PointerPhase::Up, at), &mut state);
        assert_eq!(state.last, Some(3));

        let disabled = w.item_rect(4).unwrap().center();
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Down, disabled),
            &mut state,
        );
        dispatch(
            &mut w,
            size,
            &pointer(PointerPhase::Up, disabled),
            &mut state,
        );
        assert_eq!(state.count, 1, "the disabled entry reported nothing");
    }
}
