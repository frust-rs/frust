//! Ports beUI's `bouncy-accordion` component.
//!
//! **Source:** `components/motion/bouncy-accordion.tsx` of the beUI monorepo,
//! rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! | upstream | here |
//! |---|---|
//! | row `overflow-hidden bg-card text-card-foreground` | the card fill, clipped to the row |
//! | trigger `min-h-[54px] gap-4 px-5 text-left` | [`ACCORDION_HEADER_HEIGHT`], [`ACCORDION_HEADER_GAP`], [`ACCORDION_PADDING_X`] |
//! | title `text-[15px] font-medium text-foreground` | [`ACCORDION_TITLE_SIZE`] |
//! | chevron `h-6 w-6 text-muted-foreground`, `rotate: open ? 180 : 0` | [`ACCORDION_CHEVRON_BOX`], the reveal-driven rotation |
//! | content `style={{ height: open ? contentHeight : 0 }}` `overflow-hidden` | the clipped band, `reveal · natural height` |
//! | content inner `px-5 pb-5` | [`ACCORDION_PADDING_X`], [`ACCORDION_CONTENT_PADDING_BOTTOM`] |
//! | `borderRadius: startsGroup/endsGroup ? 28 : 0` | [`ACCORDION_RADIUS`], per-corner and animated |
//! | `marginTop: separatedFromPrevious ? 12 : 0` | [`ACCORDION_SEPARATION`], animated |
//! | `ROW_TRANSITION` / `CONTENT_OPEN` / `CONTENT_CLOSE` / `CHEVRON` | [`ACCORDION_ROW_SPRING`] and friends |
//! | `item.disabled` `opacity-50` | [`style::DISABLED_OPACITY`] |
//!
//! # Motion's `{ duration, bounce }` springs, converted
//!
//! Four of upstream's five transitions are springs authored in Motion's
//! *perceptual* form — `{ type: "spring", duration, bounce }` — rather than as
//! mass/stiffness/damping. [`SpringDescription`] has no such form, so each one is
//! converted once, here, by the identity Motion itself uses for the damping
//! ratio plus this crate's own settle definition for the frequency:
//!
//! ```text
//! ζ  = 1 − bounce
//! ω₀ = −ln(ε) / (ζ · duration)      with ε = crate::motion's settle epsilon (1e-3)
//! mass = 1, stiffness = ω₀², damping = 2 · ζ · ω₀
//! ```
//!
//! The second line is chosen so that [`Ramp::settle`](crate::motion::Ramp::settle)
//! of the converted spring **equals upstream's stated duration** — the catalog's
//! settle rule is `−ln(ε) / (ζ·ω₀)`, so solving it for `ω₀` is what makes a
//! `duration: 0.55` spring here take 550ms there. The bounce is preserved
//! exactly (`ζ = 1 − bounce` is Motion's own mapping), so the overshoot is the
//! original's; only the *unit* of the number changed. A test pins both halves.
//!
//! # The height reveal, and why `paint` asks for layout
//!
//! The content child is **always laid out at its full natural height** and stays
//! laid out throughout; the reveal is a `0 → 1` fraction of that measured height
//! applied two ways — the row's reported height is `header + fraction · content`
//! (layout), and the content is drawn under a clip of that same band (paint). So
//! the animation never depends on the child's content or re-measures it
//! mid-flight, which is the same construction `frust_glyph::accordion` uses.
//!
//! That construction is also the answer to the layout-skip hazard this component
//! was flagged for. The reveal advances on a clock only `paint` has, and the
//! height is computed from it in `layout`, so a paint-only `request_frame` would
//! let an expanding panel freeze: `paint` calls `PaintCtx::request_layout` while
//! any row is still moving **and** once more on the frame a reveal actually
//! changed, which covers the landing frame and the `reduce_motion` snap (neither
//! of which reports "still animating"). No framework change was needed.
//!
//! Because the springs are the bouncy ones upstream authored, a reveal
//! **overshoots past 1** and settles back: an opening panel bounces a little
//! past its content height. The band is clipped, so the overshoot shows as the
//! row's own height bouncing, never as content painted outside it.
//!
//! # Single-open, and collapsible
//!
//! Upstream's `value` is `string | null`: at most one panel is open, and
//! `collapsible` (default `true`) decides whether pressing the open one closes
//! it. Both are kept. The port is **controlled** — a press reports the requested
//! value and the widget never writes its own.
//!
//! # Degradations against the web original
//!
//! - **No per-item icon slot.** Upstream's `item.icon` is a `7×7` leading
//!   `ReactNode`; every item here is title + chevron, and an icon would need a
//!   second child pod per row for a decoration.
//! - **No `ResizeObserver`.** Upstream watches the content for size changes and
//!   re-reads its height. Here the natural height is re-measured by every layout
//!   pass anyway, so a content that grows is picked up on the next pass without
//!   an observer — but a growth *during* a reveal keeps the height it was
//!   measured at for the rest of that flight.
//! - **The description's own fade is folded into the reveal.** Upstream fades the
//!   inner block on a separate 0.18s `EASE_OUT` ramp
//!   ([`ACCORDION_CONTENT_FADE`] keeps the number) while the band animates on the
//!   spring; here one clamped reveal drives both, so the content reaches full
//!   opacity when the band does rather than slightly before.
//! - **Three of the four springs are recorded, one is played.** Upstream gives
//!   the corner radii, the separation gap and the chevron rotation timelines of
//!   their own ([`ACCORDION_ROW_SPRING`], [`ACCORDION_CHEVRON_SPRING`]); here all
//!   three are *graded by the content reveal* instead, so they arrive with the
//!   panel rather than on their own clocks. Running four springs per row against
//!   one clamped `open_amount` would let the radius and the height disagree
//!   mid-flight — a row whose corners have rounded before its content has
//!   started. The constants stay published, converted and pinned, for a caller
//!   that wants the separate timelines.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, ChildPod,
    Color, CornerRadii, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerPhase, Rect, Role, SemanticsCtx, Size, View, Widget, any,
    build_child, erase_callback_arg, rebuild_children, route_event, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{ChildKey, FrameTime, SpringDescription, Theme};

use crate::motion::Ramp;
use crate::press::{is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::EASE_OUT;

/// A header row's height, in logical px (`min-h-[54px]`).
pub const ACCORDION_HEADER_HEIGHT: f64 = 54.0;

/// A row's horizontal padding, in logical px (`px-5`).
pub const ACCORDION_PADDING_X: f64 = 20.0;

/// The gap between a header's title and its chevron, in logical px (`gap-4`).
pub const ACCORDION_HEADER_GAP: f64 = 16.0;

/// The content block's bottom padding, in logical px (`pb-5`).
pub const ACCORDION_CONTENT_PADDING_BOTTOM: f64 = 20.0;

/// The chevron's box, in logical px (`h-6 w-6`).
pub const ACCORDION_CHEVRON_BOX: f64 = 24.0;

/// The title's type size, in logical px (`text-[15px]`).
pub const ACCORDION_TITLE_SIZE: f64 = 15.0;

/// A grouped corner's radius, in logical px (`borderRadius: 28`).
pub const ACCORDION_RADIUS: f64 = 28.0;

/// The gap opened between an item and the one above it while either is open, in
/// logical px (`marginTop: 12`).
pub const ACCORDION_SEPARATION: f64 = 12.0;

/// `ROW_TRANSITION` — `{ duration: 0.55, bounce: 0.38 }`, converted (see the
/// [module docs](self)). Drives the corner radii and the separation gap.
pub const ACCORDION_ROW_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 410.35,
    damping: 25.12,
};

/// `CONTENT_OPEN_TRANSITION` — `{ duration: 0.58, bounce: 0.32 }`, converted.
pub const ACCORDION_OPEN_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 306.74,
    damping: 23.82,
};

/// `CONTENT_CLOSE_TRANSITION` — `{ duration: 0.46, bounce: 0.26 }`, converted.
pub const ACCORDION_CLOSE_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 411.80,
    damping: 30.03,
};

/// `CHEVRON_TRANSITION` — `{ duration: 0.42, bounce: 0.28 }`, converted.
pub const ACCORDION_CHEVRON_SPRING: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 521.80,
    damping: 32.89,
};

/// `DESCRIPTION_TRANSITION` — `{ duration: 0.18, ease: EASE_OUT }`. Kept as the
/// recorded number; see the [module docs](self)' degradations for why the
/// content's opacity rides the reveal instead.
pub const ACCORDION_CONTENT_FADE: Ramp = Ramp::eased(Duration::from_millis(180), EASE_OUT);

/// One accordion panel: its identity, its header title, and the content the
/// panel discloses.
pub struct BouncyAccordionItem<State: 'static> {
    value: String,
    title: String,
    disabled: bool,
    content: AnyView<State>,
}

/// Create a panel identified by `value`, headed by `title`, disclosing
/// `content`.
pub fn bouncy_accordion_item<State: 'static, V: View<State>>(
    value: impl Into<String>,
    title: impl Into<String>,
    content: V,
) -> BouncyAccordionItem<State> {
    BouncyAccordionItem {
        value: value.into(),
        title: title.into(),
        disabled: false,
        content: any(content),
    }
}

impl<State: 'static> BouncyAccordionItem<State> {
    /// Make this panel inert: dimmed and unpressable (`item.disabled`).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The value this panel opens under.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// A view-held toggle callback, erased on build. The argument is the value the
/// press asked to open, or `None` for "close the open one".
type OnValueChange<State> = Rc<dyn Fn(&mut State, Option<String>)>;

/// A declarative beUI bouncy accordion. See the [module docs](self).
pub struct BouncyAccordionView<State: 'static> {
    value: Option<String>,
    items: Vec<BouncyAccordionItem<State>>,
    collapsible: bool,
    on_value_change: OnValueChange<State>,
}

/// Create an accordion whose open panel is the one whose value equals `value`,
/// reporting a requested value through `on_value_change` — a **controlled**
/// component.
pub fn bouncy_accordion<State: 'static, F: Fn(&mut State, Option<String>) + 'static>(
    value: Option<String>,
    items: Vec<BouncyAccordionItem<State>>,
    on_value_change: F,
) -> BouncyAccordionView<State> {
    BouncyAccordionView {
        value,
        items,
        collapsible: true,
        on_value_change: Rc::new(on_value_change),
    }
}

impl<State: 'static> BouncyAccordionView<State> {
    /// Whether pressing the open panel closes it (upstream's `collapsible`,
    /// default `true`).
    pub fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }
}

/// The resolved accordion palette.
struct AccordionColors {
    /// The row surface (`bg-card`).
    card: Color,
    /// The title ink (`text-foreground`).
    title: Color,
    /// The chevron ink (`text-muted-foreground`).
    chevron: Color,
}

/// Resolve the palette, falling back to the vendored **light** table unthemed.
fn resolve_colors(theme: Option<&Theme>) -> AccordionColors {
    match theme {
        Some(theme) => {
            let s = theme.scheme();
            AccordionColors {
                card: s.surface_container,
                title: s.on_surface,
                chevron: s.on_surface_variant,
            }
        }
        None => {
            let p = crate::BEUI_LIGHT;
            AccordionColors {
                card: p.card,
                title: p.foreground,
                chevron: p.muted_foreground,
            }
        }
    }
}

/// The header title style (`text-[15px] font-medium`), in the theme's
/// `title_small` family.
fn title_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(ACCORDION_TITLE_SIZE as f32, Color::BLACK)
    };
    themed_style(style, ThemeTextType::TitleSmall, theme)
}

/// One row's reveal: where it is, where it is going, and the ramp between.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reveal {
    /// The displayed fraction, `0` closed to `1` open (a spring overshoots past
    /// `1` on the way in).
    value: f64,
    /// The fraction the current flight started from, if one is running.
    from: Option<f64>,
    /// The fraction it is heading for.
    target: f64,
    /// The ramp this flight plays.
    ramp: Ramp,
    started: Option<FrameTime>,
}

impl Reveal {
    fn at_rest(open: bool) -> Self {
        Reveal {
            value: if open { 1.0 } else { 0.0 },
            from: None,
            target: if open { 1.0 } else { 0.0 },
            ramp: Ramp::spring(ACCORDION_OPEN_SPRING),
            started: None,
        }
    }

    /// Retarget toward `open`, leaving from wherever the row is displaying.
    fn retarget(&mut self, open: bool) {
        let target = if open { 1.0 } else { 0.0 };
        if self.target == target {
            return;
        }
        self.from = Some(self.value);
        self.target = target;
        self.ramp = Ramp::spring(if open {
            ACCORDION_OPEN_SPRING
        } else {
            ACCORDION_CLOSE_SPRING
        });
        self.started = None;
    }

    /// Step to `now`, returning whether this row is still moving.
    fn advance(&mut self, now: FrameTime, reduce_motion: bool) -> bool {
        if reduce_motion {
            self.from = None;
            self.started = None;
            self.value = self.target;
            return false;
        }
        let Some(from) = self.from else {
            self.value = self.target;
            return false;
        };
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if self.ramp.is_settled(elapsed) {
            self.from = None;
            self.started = None;
            self.value = self.target;
            return false;
        }
        // Raw progress: an opening panel bounces past its content height, which
        // is what upstream's `bounce: 0.32` buys.
        self.value = (from + (self.target - from) * self.ramp.progress(elapsed)).max(0.0);
        true
    }
}

/// One retained panel.
struct Entry {
    value: String,
    title: LabelRun,
    title_text: String,
    disabled: bool,
    reveal: Reveal,
    /// The content child's natural height, measured every layout pass.
    content_height: f64,
}

impl<State: 'static> View<State> for BouncyAccordionView<State> {
    type Element = BouncyAccordionWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> BouncyAccordionWidget {
        BouncyAccordionWidget {
            entries: self
                .items
                .iter()
                .map(|item| Entry {
                    value: item.value.clone(),
                    title: LabelRun::new(item.title.clone()),
                    title_text: item.title.clone(),
                    disabled: item.disabled,
                    reveal: Reveal::at_rest(self.value.as_deref() == Some(item.value.as_str())),
                    content_height: 0.0,
                })
                .collect(),
            contents: self
                .items
                .iter()
                .map(|item| build_child(&item.content, ctx))
                .collect(),
            value: self.value.clone(),
            collapsible: self.collapsible,
            width: 0.0,
            focused: 0,
            hovered: None,
            captured: None,
            on_value_change: erase_callback_arg(&self.on_value_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut BouncyAccordionWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_value_change = erase_callback_arg(&self.on_value_change);
        let mut flags = ChangeFlags::NONE;

        if prev.items.len() != self.items.len() {
            element.entries = self
                .items
                .iter()
                .map(|item| Entry {
                    value: item.value.clone(),
                    title: LabelRun::new(item.title.clone()),
                    title_text: item.title.clone(),
                    disabled: item.disabled,
                    reveal: Reveal::at_rest(self.value.as_deref() == Some(item.value.as_str())),
                    content_height: 0.0,
                })
                .collect();
            element.captured = None;
            element.hovered = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        } else {
            for (entry, item) in element.entries.iter_mut().zip(self.items.iter()) {
                if entry.title_text != item.title {
                    entry.title = LabelRun::new(item.title.clone());
                    entry.title_text = item.title.clone();
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                if entry.value != item.value || entry.disabled != item.disabled {
                    entry.value = item.value.clone();
                    entry.disabled = item.disabled;
                    flags |= ChangeFlags::PAINT;
                }
            }
        }

        if prev.value != self.value {
            element.value = self.value.clone();
            let open = element.value.clone();
            for entry in &mut element.entries {
                entry
                    .reveal
                    .retarget(open.as_deref() == Some(entry.value.as_str()));
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.collapsible != self.collapsible {
            element.collapsible = self.collapsible;
        }

        flags |= rebuild_children(
            &prev.items,
            &self.items,
            &mut element.contents,
            ctx,
            |item: &BouncyAccordionItem<State>| &item.content,
            |_| None::<ChildKey>,
        );
        element.focused = element.focused.min(element.entries.len().saturating_sub(1));
        flags
    }

    fn teardown(&self, element: &mut BouncyAccordionWidget, ctx: &mut BuildCtx<'_>) {
        for (item, pod) in self.items.iter().zip(element.contents.iter_mut()) {
            teardown_child(&item.content, pod, ctx);
        }
    }
}

/// The retained widget for a [`BouncyAccordionView`].
pub struct BouncyAccordionWidget {
    entries: Vec<Entry>,
    /// One content pod per panel, always laid out at its natural height.
    contents: Vec<ChildPod>,
    /// The app-confirmed open value (source of truth, adopted on `rebuild`).
    value: Option<String>,
    collapsible: bool,
    /// The width layout resolved.
    width: f64,
    /// The header the roving cursor sits on.
    focused: usize,
    /// The latched hovered header, self-corrected from `PaintCtx::is_hovered`.
    hovered: Option<usize>,
    /// The header a `Down` armed.
    captured: Option<usize>,
    on_value_change: frust::authoring::ErasedArgCallback<Option<String>>,
}

impl BouncyAccordionWidget {
    /// The open panel's index, if any.
    fn open_index(&self) -> Option<usize> {
        let value = self.value.as_deref()?;
        self.entries.iter().position(|e| e.value == value)
    }

    /// Whether panel `index` can be pressed.
    fn enabled(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|e| !e.disabled)
    }

    /// How open panel `index` reads right now, clamped into `[0, 1]` — the
    /// quantity every grouping decision is graded by.
    fn open_amount(&self, index: usize) -> f64 {
        self.entries
            .get(index)
            .map_or(0.0, |e| e.reveal.value.clamp(0.0, 1.0))
    }

    /// The gap above panel `index` (`marginTop: separatedFromPrevious ? 12 : 0`,
    /// graded so it opens with the panel rather than jumping).
    fn separation(&self, index: usize) -> f64 {
        if index == 0 {
            return 0.0;
        }
        ACCORDION_SEPARATION * self.open_amount(index).max(self.open_amount(index - 1))
    }

    /// Panel `index`'s content band height.
    fn band_height(&self, index: usize) -> f64 {
        self.entries
            .get(index)
            .map_or(0.0, |e| (e.reveal.value * e.content_height).max(0.0))
    }

    /// Panel `index`'s whole row height: header plus band.
    fn row_height(&self, index: usize) -> f64 {
        ACCORDION_HEADER_HEIGHT + self.band_height(index)
    }

    /// Panel `index`'s row box.
    fn row_rect(&self, index: usize) -> Option<Rect> {
        if index >= self.entries.len() {
            return None;
        }
        let mut y = 0.0;
        for i in 0..index {
            y += self.separation(i) + self.row_height(i);
        }
        y += self.separation(index);
        Some(Rect::from_origin_size(
            Point::new(0.0, y),
            Size::new(self.width, self.row_height(index)),
        ))
    }

    /// Panel `index`'s corner radii (`startsGroup`/`endsGroup`, graded).
    fn radii(&self, index: usize) -> CornerRadii {
        let last = self.entries.len().saturating_sub(1);
        let own = self.open_amount(index);
        let starts = if index == 0 {
            1.0
        } else {
            own.max(self.open_amount(index - 1))
        };
        let ends = if index == last {
            1.0
        } else {
            own.max(self.open_amount(index + 1))
        };
        CornerRadii::new(
            ACCORDION_RADIUS * starts,
            ACCORDION_RADIUS * starts,
            ACCORDION_RADIUS * ends,
            ACCORDION_RADIUS * ends,
        )
    }

    /// The header under a widget-local `pos`, if any (the header band only).
    fn hit_header(&self, pos: Point) -> Option<usize> {
        (0..self.entries.len()).find(|index| {
            self.row_rect(*index).is_some_and(|rect| {
                pos.y >= rect.y0
                    && pos.y < rect.y0 + ACCORDION_HEADER_HEIGHT
                    && pos.x >= rect.x0
                    && pos.x < rect.x1
            })
        })
    }

    /// Report the value panel `index` asks for: itself, or `None` when it is
    /// already open and the accordion is collapsible.
    fn request_toggle(&mut self, ctx: &mut EventCtx, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if entry.disabled {
            return;
        }
        let value = entry.value.clone();
        let already_open = self.value.as_deref() == Some(value.as_str());
        let requested = if already_open {
            if !self.collapsible {
                return;
            }
            None
        } else {
            Some(value)
        };
        (self.on_value_change)(ctx, requested);
    }

    /// The next enabled header `step` places from `from`, wrapping.
    fn step_enabled(&self, from: usize, step: isize) -> Option<usize> {
        let len = self.entries.len();
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
}

/// Paint a chevron pointing down, rotated `angle` radians about `centre`.
fn draw_chevron(scene: &mut dyn PaintScene, centre: Point, angle: f64, color: Color) {
    let arm = ACCORDION_CHEVRON_BOX * 0.18;
    let mut path = BezPath::new();
    path.move_to(Point::new(-arm, -arm * 0.6));
    path.line_to(Point::new(0.0, arm * 0.6));
    path.line_to(Point::new(arm, -arm * 0.6));
    scene.push_transform(Affine::translate(centre.to_vec2()) * Affine::rotate(angle));
    scene.stroke_path(Point::ZERO, &path, 1.75, &Brush::Solid(color));
    scene.pop_transform();
}

impl Widget for BouncyAccordionWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = title_style(Theme::from_layout_ctx(ctx));
        self.width = bc.max().width;

        // The content is always measured at its full natural height and stays
        // laid out, so the reveal never re-measures it mid-flight.
        let content_bc = BoxConstraints::new(
            Size::ZERO,
            Size::new(
                (self.width - ACCORDION_PADDING_X * 2.0).max(0.0),
                bc.max().height,
            ),
        );
        for index in 0..self.entries.len() {
            self.entries[index].title.layout(ctx, &style);
            let Some(pod) = self.contents.get_mut(index) else {
                continue;
            };
            let size = pod.layout_child(ctx, &content_bc);
            self.entries[index].content_height = size.height + ACCORDION_CONTENT_PADDING_BOTTOM;
        }
        for index in 0..self.entries.len() {
            let Some(rect) = self.row_rect(index) else {
                continue;
            };
            if let Some(pod) = self.contents.get_mut(index) {
                pod.set_origin(Point::new(
                    rect.x0 + ACCORDION_PADDING_X,
                    rect.y0 + ACCORDION_HEADER_HEIGHT,
                ));
            }
        }

        let height = (0..self.entries.len())
            .map(|index| self.separation(index) + self.row_height(index))
            .sum::<f64>();
        bc.constrain(Size::new(self.width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if !ctx.is_hovered() {
            self.hovered = None;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let colors = resolve_colors(theme);
        let reduce_motion = theme.is_some_and(|t| t.motion.reduce_motion);
        let origin = ctx.origin();

        let before: Vec<f64> = self.entries.iter().map(|e| e.reveal.value).collect();
        let mut running = false;
        let now = ctx.frame_time();
        for entry in &mut self.entries {
            running |= entry.reveal.advance(now, reduce_motion);
        }

        for index in 0..self.entries.len() {
            let Some(rect) = self.row_rect(index) else {
                continue;
            };
            let dimmed = !self.enabled(index);
            let row_origin = Point::new(origin.x + rect.x0, origin.y + rect.y0);
            let row_size = Size::new(rect.width(), rect.height());

            // The row card, with its grouped corners.
            scene.fill_rounded_rect_radii(
                row_origin,
                row_size,
                self.radii(index),
                style::disabled_tint(colors.card, dimmed, style::DISABLED_OPACITY),
            );

            let entry = &self.entries[index];
            let hovered = self.hovered == Some(index);
            let title_ink = style::disabled_tint(colors.title, dimmed, style::DISABLED_OPACITY);
            let chevron_ink = style::disabled_tint(
                if hovered {
                    colors.title
                } else {
                    colors.chevron
                },
                dimmed,
                style::DISABLED_OPACITY,
            );

            let title = entry.title.size();
            entry.title.paint(
                Point::new(
                    row_origin.x + ACCORDION_PADDING_X,
                    row_origin.y + (ACCORDION_HEADER_HEIGHT - title.height) / 2.0,
                ),
                title_ink,
                scene,
            );

            // The chevron turns a full 180° as the panel opens (`rotate: 180`).
            draw_chevron(
                scene,
                Point::new(
                    row_origin.x + rect.width() - ACCORDION_PADDING_X + ACCORDION_HEADER_GAP / 2.0
                        - ACCORDION_CHEVRON_BOX / 2.0
                        - ACCORDION_HEADER_GAP / 2.0,
                    row_origin.y + ACCORDION_HEADER_HEIGHT / 2.0,
                ),
                self.open_amount(index) * std::f64::consts::PI,
                chevron_ink,
            );

            // The content, under a clip of exactly the revealed band.
            let band = self.band_height(index);
            if band > 0.5 {
                let alpha = self.open_amount(index) as f32;
                scene.push_clip(
                    Point::new(row_origin.x, row_origin.y + ACCORDION_HEADER_HEIGHT),
                    Size::new(rect.width(), band),
                );
                if alpha < 1.0 {
                    scene.push_layer(
                        Point::new(row_origin.x, row_origin.y + ACCORDION_HEADER_HEIGHT),
                        Size::new(rect.width(), band),
                        alpha,
                    );
                }
                if let Some(pod) = self.contents.get_mut(index) {
                    pod.paint_child(ctx, scene);
                }
                if alpha < 1.0 {
                    scene.pop_layer();
                }
                scene.pop_clip();
            }
        }

        // The row heights are computed in `layout` from the reveals, so a bare
        // frame request would let an expanding panel freeze on the intra-frame
        // layout skip. Ask for layout while any row moves, and once more on the
        // frame a value actually changed — which covers the landing frame and
        // the `reduce_motion` snap, neither of which reports "still animating".
        if running {
            ctx.request_layout();
        }
        if self
            .entries
            .iter()
            .zip(before.iter())
            .any(|(entry, was)| entry.reveal.value != *was)
        {
            ctx.request_layout();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            for pod in &mut self.contents {
                pod.event_child(ctx, event);
            }
            return EventResult::Ignored;
        }
        // The revealed content is routed first, so an interactive panel body
        // wins over the header strip's own fallback handling.
        if route_event(&mut self.contents, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }

        match event {
            InputEvent::Key(key) => {
                let step = match &key.key {
                    Key::Named(NamedKey::ArrowDown) => Some(1),
                    Key::Named(NamedKey::ArrowUp) => Some(-1),
                    _ => None,
                };
                if let Some(step) = step {
                    let Some(next) = self.step_enabled(self.focused, step) else {
                        return EventResult::Ignored;
                    };
                    self.focused = next;
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) {
                    let index = self.focused;
                    self.request_toggle(ctx, index);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    if !presses(p) {
                        return EventResult::Ignored;
                    }
                    let Some(index) = self.hit_header(p.position).filter(|i| self.enabled(*i))
                    else {
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
                    let over = self.hit_header(p.position).filter(|i| self.enabled(*i));
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
                    if self.hit_header(p.position) == Some(armed) {
                        self.request_toggle(ctx, armed);
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
        let open = self.open_index();
        for (index, entry) in self.entries.iter().enumerate() {
            ctx.push_node(Role::Button, |node| {
                node.set_label(entry.title_text.as_str());
                node.set_expanded(open == Some(index));
                if entry.disabled {
                    node.set_disabled();
                } else {
                    node.add_action(Action::Click);
                }
            });
        }
        // Only the open panel's body is published: a closed one is `inert` and
        // `aria-hidden` upstream, and here it is not painted or routed to.
        if let Some(pod) = open.and_then(|index| self.contents.get(index)) {
            pod.semantics_child(ctx);
        }
    }

    visit_children!(contents);
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{KeyEvent, Modifiers, PointerButton, PointerEvent};
    use frust::text;
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        radii: Vec<(Point, Size, CornerRadii)>,
        clips: Vec<(Point, Size)>,
        layers: Vec<f32>,
        inks: Vec<Color>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _c: Color) {}
        fn fill_rounded_rect_radii(&mut self, o: Point, s: Size, radii: CornerRadii, _c: Color) {
            self.radii.push((o, s, radii));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_clip(&mut self, o: Point, s: Size) {
            self.clips.push((o, s));
        }
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
        last: Option<Option<String>>,
        count: u32,
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn items() -> Vec<BouncyAccordionItem<Picked>> {
        vec![
            bouncy_accordion_item("shipping", "Shipping", text("Ships in two days.")),
            bouncy_accordion_item("returns", "Returns", text("Thirty day window.")),
            bouncy_accordion_item("support", "Support", text("Weekdays, 9 to 5.")),
            bouncy_accordion_item("legal", "Legal", text("Nothing to see.")).disabled(true),
        ]
    }

    fn view(value: Option<&str>, collapsible: bool) -> BouncyAccordionView<Picked> {
        bouncy_accordion::<Picked, _>(
            value.map(str::to_owned),
            items(),
            |s: &mut Picked, v: Option<String>| {
                s.last = Some(v);
                s.count += 1;
            },
        )
        .collapsible(collapsible)
    }

    fn build(value: Option<&str>, collapsible: bool) -> BouncyAccordionWidget {
        let mut counter = 0u64;
        View::<Picked>::build(&view(value, collapsible), &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut BouncyAccordionWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(
            &mut ctx,
            &BoxConstraints::new(Size::ZERO, Size::new(420.0, 900.0)),
        )
    }

    fn laid_out(value: Option<&str>, collapsible: bool) -> (BouncyAccordionWidget, Size) {
        let mut w = build(value, collapsible);
        let size = layout(&mut w);
        (w, size)
    }

    fn paint_at(
        w: &mut BouncyAccordionWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: f64,
    ) -> (Recorder, bool, bool) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        if let Some(t) = theme {
            ctx = ctx.with_theme(t);
        }
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame(), ctx.needs_layout())
    }

    fn open(
        w: &mut BouncyAccordionWidget,
        from: Option<&str>,
        to: Option<&str>,
        collapsible: bool,
    ) {
        let mut counter = 0u64;
        let mut ctx = BuildCtx::new(&mut counter);
        View::<Picked>::rebuild(
            &view(to, collapsible),
            &view(from, collapsible),
            w,
            &mut ctx,
        );
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

    fn dispatch(w: &mut BouncyAccordionWidget, size: Size, event: &InputEvent, state: &mut Picked) {
        let mut ctx = EventCtx::new(state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// Press the header of panel `index`.
    fn press(w: &mut BouncyAccordionWidget, size: Size, index: usize, state: &mut Picked) {
        let rect = w.row_rect(index).unwrap();
        let at = Point::new(rect.x0 + 10.0, rect.y0 + ACCORDION_HEADER_HEIGHT / 2.0);
        dispatch(w, size, &pointer(PointerPhase::Down, at), state);
        dispatch(w, size, &pointer(PointerPhase::Up, at), state);
    }

    /// The four converted springs keep upstream's `duration` as their settle
    /// time and its `bounce` as their damping ratio — the whole content of the
    /// conversion, pinned.
    #[test]
    fn the_converted_springs_keep_upstream_duration_and_bounce() {
        for (spring, duration, bounce) in [
            (ACCORDION_ROW_SPRING, 0.55, 0.38),
            (ACCORDION_OPEN_SPRING, 0.58, 0.32),
            (ACCORDION_CLOSE_SPRING, 0.46, 0.26),
            (ACCORDION_CHEVRON_SPRING, 0.42, 0.28),
        ] {
            let zeta = spring.damping / (2.0 * (spring.mass * spring.stiffness).sqrt());
            assert!(
                (zeta - (1.0 - bounce)).abs() < 0.005,
                "{spring:?} has damping ratio {zeta}, wanted {}",
                1.0 - bounce
            );
            let settle = Ramp::spring(spring).settle().as_secs_f64();
            assert!(
                (settle - duration).abs() < 0.01,
                "{spring:?} settles in {settle}s, wanted {duration}s"
            );
        }
    }

    /// At most one panel is open, and opening a second closes the first — the
    /// exclusivity `value: string | null` buys.
    #[test]
    fn opening_a_second_panel_closes_the_first() {
        let (mut w, size) = laid_out(Some("shipping"), true);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.entries[0].reveal.value, 1.0);
        assert_eq!(w.entries[1].reveal.value, 0.0);

        open(&mut w, Some("shipping"), Some("returns"), true);
        // Both rows move: one closes as the other opens.
        assert_eq!(w.entries[0].reveal.target, 0.0);
        assert_eq!(w.entries[1].reveal.target, 1.0);
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 200.0);
        assert!(w.entries[0].reveal.value < 1.0 && w.entries[1].reveal.value > 0.0);

        paint_at(&mut w, size, None, 2_000.0);
        assert_eq!(w.entries[0].reveal.value, 0.0);
        assert_eq!(w.entries[1].reveal.value, 1.0);
    }

    /// A press reports the value it wants and never writes it; pressing the open
    /// panel asks to close it, unless the accordion is not collapsible.
    #[test]
    fn a_press_reports_the_requested_value() {
        let (mut w, size) = laid_out(None, true);
        let mut state = Picked::default();
        press(&mut w, size, 1, &mut state);
        assert_eq!(state.last, Some(Some("returns".to_owned())));
        assert_eq!(w.value, None, "the widget never writes its own value");

        let (mut w, size) = laid_out(Some("returns"), true);
        let mut state = Picked::default();
        press(&mut w, size, 1, &mut state);
        assert_eq!(state.last, Some(None), "collapsible closes on a re-press");

        let (mut w, size) = laid_out(Some("returns"), false);
        let mut state = Picked::default();
        press(&mut w, size, 1, &mut state);
        assert_eq!(state.count, 0, "a non-collapsible one reports nothing");
    }

    /// A disabled panel is inert to the pointer and skipped by the arrows.
    #[test]
    fn a_disabled_panel_is_inert() {
        let (mut w, size) = laid_out(None, true);
        let mut state = Picked::default();
        press(&mut w, size, 3, &mut state);
        assert_eq!(state.count, 0);

        w.focused = 2;
        dispatch(&mut w, size, &key_event(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, 0, "the arrows wrap past the disabled panel");
    }

    /// The reveal really does animate the *layout*: the widget grows over the
    /// flight, every frame asks for relayout, and the band is clipped to exactly
    /// the revealed height. This is the layout-skip hazard the port was flagged
    /// for, and it is handled without touching framework code.
    #[test]
    fn expanding_grows_the_widget_and_asks_for_relayout() {
        let (mut w, size) = laid_out(None, true);
        let closed = layout(&mut w).height;
        paint_at(&mut w, size, None, 0.0);

        open(&mut w, None, Some("shipping"), true);
        let (_, _, needs_layout) = paint_at(&mut w, size, None, 100.0);
        assert!(needs_layout, "an expanding panel must ask for relayout");

        // Not a monotone growth — the spring is a bouncy one, so the height
        // passes its resting value and comes back. What must hold is that it
        // leaves the closed height, that the clip always equals the band, and
        // that it lands where layout says it should.
        assert_eq!(
            layout(&mut w).height,
            closed,
            "the first frame is still closed"
        );
        let mut clipped_band = 0.0f64;
        let mut tallest = closed;
        for step in 1..=12 {
            let (rec, _, _) = paint_at(&mut w, size, None, 100.0 + step as f64 * 30.0);
            tallest = tallest.max(layout(&mut w).height);
            if let Some((_, size)) = rec.clips.first() {
                clipped_band = clipped_band.max(size.height);
                assert!(
                    (size.height - w.band_height(0)).abs() < 1e-9,
                    "the clip and the band disagree"
                );
            }
        }
        assert!(clipped_band > 0.0, "the content band never opened");
        assert!(tallest > closed, "the accordion never grew");

        paint_at(&mut w, size, None, 5_000.0);
        let (_, needs_frame, still) = paint_at(&mut w, size, None, 5_100.0);
        assert!(
            !needs_frame && !still,
            "a settled accordion asks for nothing"
        );
        assert_eq!(
            layout(&mut w).height,
            closed + w.entries[0].content_height + ACCORDION_SEPARATION,
            "the open row is header + content, and it separated from its neighbour"
        );
    }

    /// The opening spring overshoots, so the panel bounces past its content
    /// height before settling on it — the component's whole name.
    #[test]
    fn the_opening_panel_bounces_past_its_content_height() {
        let (mut w, size) = laid_out(None, true);
        paint_at(&mut w, size, None, 0.0);
        open(&mut w, None, Some("shipping"), true);

        let mut peak = 0.0f64;
        for step in 0..=60 {
            paint_at(&mut w, size, None, 100.0 + step as f64 * 10.0);
            peak = peak.max(w.entries[0].reveal.value);
        }
        assert!(peak > 1.0, "the panel never overshot: {peak}");
        assert_eq!(w.entries[0].reveal.value, 1.0, "and it settled on target");
    }

    /// The grouping radii and the separation gap are graded by how open a panel
    /// is: closed, the stack is one flush block with only its outer corners
    /// rounded; open, the panel and its neighbours round and part.
    #[test]
    fn the_group_corners_and_the_separation_follow_the_open_panel() {
        let (mut w, size) = laid_out(None, true);
        paint_at(&mut w, size, None, 0.0);
        assert_eq!(w.radii(0).top_left, ACCORDION_RADIUS, "the first row's top");
        assert_eq!(w.radii(1).top_left, 0.0, "a middle row is flush");
        assert_eq!(
            w.radii(3).bottom_left,
            ACCORDION_RADIUS,
            "the last row's foot"
        );
        assert_eq!(w.separation(1), 0.0);

        open(&mut w, None, Some("returns"), true);
        paint_at(&mut w, size, None, 100.0);
        paint_at(&mut w, size, None, 5_000.0);
        assert_eq!(w.radii(1).top_left, ACCORDION_RADIUS, "the open row rounds");
        assert_eq!(
            w.radii(0).bottom_left,
            ACCORDION_RADIUS,
            "and so does the row above it, which now ends a group"
        );
        assert_eq!(w.separation(1), ACCORDION_SEPARATION);
        assert_eq!(w.separation(2), ACCORDION_SEPARATION);
    }

    /// `reduce_motion` lands both rows on the frame the toggle is painted, and
    /// asks for the one relayout that publishes the new heights.
    #[test]
    fn reduce_motion_lands_the_toggle_immediately() {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        let (mut w, size) = laid_out(Some("shipping"), true);
        paint_at(&mut w, size, Some(&theme), 0.0);
        open(&mut w, Some("shipping"), Some("returns"), true);

        let (_, _, needs_layout) = paint_at(&mut w, size, Some(&theme), 100.0);
        assert!(needs_layout, "the snap still publishes its new heights");
        assert_eq!(w.entries[0].reveal.value, 0.0);
        assert_eq!(w.entries[1].reveal.value, 1.0);

        layout(&mut w);
        let (_, needs_frame, still) = paint_at(&mut w, size, Some(&theme), 120.0);
        assert!(!needs_frame && !still);
    }

    /// The paint smoke: an open accordion paints one card per row, one title per
    /// row, a rotated chevron per row, and exactly one clipped content band.
    #[test]
    fn an_open_accordion_paints_its_whole_chrome() {
        let (mut w, size) = laid_out(Some("shipping"), true);
        let (rec, _, _) = paint_at(&mut w, size, None, 0.0);
        assert_eq!(rec.radii.len(), 4, "one card per row");
        assert_eq!(rec.transforms.len(), 4, "one chevron transform per row");
        assert_eq!(rec.clips.len(), 1, "only the open row bands its content");
        assert!(rec.clips[0].1.height > 0.0, "the open band has real height");
        // Four titles plus the open panel's own text child.
        assert_eq!(rec.inks.len(), 5);
        assert_eq!(rec.inks[0], crate::BEUI_LIGHT.foreground);
    }

    /// Keyboard: the arrows move the roving cursor and `Enter` toggles the panel
    /// under it.
    #[test]
    fn the_arrows_move_the_cursor_and_enter_toggles() {
        let (mut w, size) = laid_out(None, true);
        let mut state = Picked::default();
        dispatch(&mut w, size, &key_event(NamedKey::ArrowDown), &mut state);
        assert_eq!(w.focused, 1);
        dispatch(&mut w, size, &key_event(NamedKey::Enter), &mut state);
        assert_eq!(state.last, Some(Some("returns".to_owned())));
    }

    // ---- Typeface: the header titles follow the live theme -------------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(400.0, 600.0);

    /// Two collapsed rows over glyph-free panels, so every painted run is a
    /// header title.
    fn probe_view(_: &mut ()) -> BouncyAccordionView<()> {
        let panel = || frust::SizedBox::<()>(Some(40.0), Some(40.0));
        bouncy_accordion::<(), _>(
            None,
            vec![
                bouncy_accordion_item("shipping", "Shipping", panel()),
                bouncy_accordion_item("returns", "Returns", panel()),
            ],
            |_: &mut (), _| {},
        )
    }

    #[test]
    fn header_titles_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the accordion's titles", probe_view, PROBE_WINDOW);
    }

    #[test]
    fn header_titles_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the accordion's titles", probe_view, PROBE_WINDOW);
    }
}
