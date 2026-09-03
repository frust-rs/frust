//! Ports beUI's `message` agent-interface part.
//!
//! **Source:** `components/agents/message.tsx` (beUI v2, rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `message`: *"Composable conversation primitives for message rows,
//! grouped bubbles, avatars, metadata, live markers, and a mount-only
//! trailing-edge pop-up for newly sent rows."*
//!
//! Where `message-bubble` is the *surface* a message is painted on, this is the
//! **row** it sits in: avatar, sender metadata, an arbitrary content slot, and a
//! footer. The content slot takes any view — a
//! [`message_bubble`](super::message_bubble::message_bubble), a
//! [`streaming_response`](super::streaming_response::streaming_response), a
//! tool card — so the row never has to know what a message *is*.
//!
//! # Upstream's exports, and where each landed
//!
//! | upstream export | here |
//! |---|---|
//! | `Message` | [`message`] — the row, with `from` deciding its direction |
//! | `MessageAvatar` | [`MessageView::avatar`] / [`MessageView::avatar_placeholder`] |
//! | `MessageHeader` | [`MessageView::name`] + [`MessageView::timestamp`] |
//! | `MessageContent` | the content slot [`message`] takes |
//! | `MessageFooter` | [`MessageView::footer`] |
//! | `MessageMarker` | [`message_marker`] |
//! | `MessageGroup` | [`message_bubble_group`](super::message_bubble::message_bubble_group) — one column primitive serves both files |
//! | `MessageTyping` | [`loading_states`](super::loading_states)' [`Dots`](super::loading_states::LoadingStatesVariant::Dots) variant — one three-dot indicator in the catalog, not two |
//!
//! Upstream's `MessageSideContext` (a React context threading `start`/`end`
//! down to a nested bubble) has no counterpart: a frust view tree has no
//! ambient context a child widget can read at layout time, so a bubble inside a
//! row is told its side explicitly by the caller.
//!
//! # Geometry
//!
//! | class | here |
//! |---|---|
//! | `flex w-full items-start gap-2` (+`flex-row-reverse` for `user`) | the row, [`MESSAGE_ROW_GAP`] apart, mirrored by [`MessageFrom`] |
//! | avatar `size-7 rounded-full bg-muted text-xs font-medium` | [`MESSAGE_AVATAR_SIZE`] |
//! | content `flex min-w-0 flex-1 flex-col gap-1.5` | [`MESSAGE_COLUMN_GAP`] |
//! | header/footer `px-1 text-[11px]` | [`MESSAGE_META_PADDING_X`] / [`MESSAGE_META_TEXT`] |
//! | footer `min-h-5` | [`MESSAGE_FOOTER_MIN_HEIGHT`] |
//! | marker `rounded-full bg-muted/70 px-2.5 py-1 text-xs max-w-[88%]` | [`message_marker`] |
//!
//! # Entrance
//!
//! `animateIn` plays `MESSAGE_POP_UP`: opacity `0 → 1`, `translateY(8px) → 0`
//! and `scale(0.95) → 1` about the row's own bottom corner on the side it came
//! from (`transformOrigin: 100% 100%` for a sent row, `0% 100%` for a received
//! one). One [`Lane`] drives all three, staged on the first paint and snapped
//! outright under `reduce_motion`.
//!
//! # Degradations against upstream
//!
//! - **`MESSAGE_POP_UP` is substituted.** Upstream authors a per-component
//!   `{ stiffness: 480, damping: 32, mass: 0.62 }`, which is not one of
//!   [`crate::tokens::motion`]'s six; `SPRING_PRESS` (500/30/0.6) is the nearest
//!   catalog spring and drives the pop instead.
//! - **No exit.** Upstream's `exit` variant needs React's `AnimatePresence`; a
//!   row here is gone the frame after the rebuild that stopped returning it.
//!   [`crate::motion::Presence`] exists for a caller that wants to keep one
//!   mounted through an exit, but the row does not mount itself.
//! - **The avatar is text, not a slot.** Upstream takes a `ReactNode` (an
//!   `<img>`, an icon). This port takes a short string — initials — and paints
//!   it in the circle; an image avatar is a caller's own composition problem
//!   until the catalog has an image primitive to hand it.
//! - **Metadata is two strings, not a slot.** `MessageHeader`/`MessageFooter`
//!   are arbitrary children upstream; here they are the name/timestamp pair and
//!   one footer string, which is what every upstream preview actually puts in
//!   them.

use frust::Theme;
use frust::authoring::text::{FontWeight, TextStyle};
use frust::authoring::{
    Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, EventCtx, EventResult,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, View, Widget,
    any, build_child, rebuild_child, route_event_single, teardown_child, visit_children,
};

use crate::motion::Ramp;
use crate::press::Lane;
use crate::style::{self, scale_alpha};
use crate::text::Label;
use crate::tokens::motion::SPRING_PRESS;
use crate::tokens::{BEUI_LIGHT, BeuiPalette, sans_family};

/// The avatar's edge, in logical px (`size-7`).
pub const MESSAGE_AVATAR_SIZE: f64 = 28.0;

/// The gap between the avatar and the content column, in logical px (`gap-2`).
pub const MESSAGE_ROW_GAP: f64 = style::SPACING_UNIT * 2.0;

/// The gap between the header, the content and the footer, in logical px
/// (`gap-1.5`).
pub const MESSAGE_COLUMN_GAP: f64 = style::SPACING_UNIT * 1.5;

/// Horizontal padding on the metadata rows, in logical px (`px-1`) — they sit a
/// hair inside the bubble's own curve.
pub const MESSAGE_META_PADDING_X: f64 = style::SPACING_UNIT;

/// The metadata type size, in logical px (`text-[11px]`).
pub const MESSAGE_META_TEXT: f64 = 11.0;

/// The footer row's reserved height, in logical px (`min-h-5`).
pub const MESSAGE_FOOTER_MIN_HEIGHT: f64 = 20.0;

/// How far a sent row rises into place, in logical px (`translateY(8px)`).
pub const MESSAGE_POP_RISE: f64 = 8.0;

/// The row's entrance scale floor (`scale(0.95)`).
pub const MESSAGE_POP_SCALE: f64 = 0.95;

/// The gap between the sender's name and the timestamp, in logical px
/// (`gap-1.5`).
const META_GAP: f64 = style::SPACING_UNIT * 1.5;

/// The largest fraction of the row a [`message_marker`] may occupy
/// (`max-w-[88%]`).
pub const MESSAGE_MARKER_MAX_WIDTH_FRACTION: f64 = 0.88;

/// Horizontal padding inside a marker, in logical px (`px-2.5`).
pub const MESSAGE_MARKER_PADDING_X: f64 = 10.0;

/// Vertical padding inside a marker, in logical px (`py-1`).
pub const MESSAGE_MARKER_PADDING_Y: f64 = style::SPACING_UNIT;

/// Alpha of a marker's fill (`bg-muted/70`).
const MARKER_FILL_ALPHA: f32 = 0.7;

/// Unthemed fallback palette — see [`super::message_bubble`]'s own note.
const FALLBACK: BeuiPalette = BEUI_LIGHT;

/// Who sent a message — upstream's `MessageFrom`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageFrom {
    /// A sent message: the row is mirrored (`flex-row-reverse`), its metadata
    /// right-aligned, and its entrance anchored to the trailing edge.
    User,
    /// A received message: the row reads leading-edge first.
    #[default]
    Assistant,
}

impl MessageFrom {
    /// Every sender, in upstream's own union order.
    pub const ALL: [MessageFrom; 2] = [MessageFrom::User, MessageFrom::Assistant];

    /// Whether this sender's row is mirrored — `flex-row-reverse` plus
    /// `items-end` metadata.
    pub const fn is_mirrored(self) -> bool {
        matches!(self, MessageFrom::User)
    }

    /// The accessible name upstream gives the row (`aria-label={`${from}
    /// message`}`).
    pub const fn aria_label(self) -> &'static str {
        match self {
            MessageFrom::User => "user message",
            MessageFrom::Assistant => "assistant message",
        }
    }
}

/// The colours one message row resolves.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MessagePaint {
    /// The avatar circle's fill (`bg-muted`).
    avatar: Color,
    /// The metadata ink (`text-muted-foreground`).
    meta: Color,
}

impl MessagePaint {
    /// Resolve against `theme`, falling back to beUI light.
    fn resolve(theme: Option<&Theme>) -> Self {
        match theme {
            Some(theme) => {
                let scheme = theme.scheme();
                MessagePaint {
                    avatar: scheme.surface_container_highest,
                    meta: scheme.on_surface_variant,
                }
            }
            None => MessagePaint {
                avatar: FALLBACK.muted,
                meta: FALLBACK.muted_foreground,
            },
        }
    }
}

/// The metadata text style: `text-[11px] leading-none text-muted-foreground`.
fn meta_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        size: MESSAGE_META_TEXT as f32,
        color,
        ..TextStyle::default()
    }
}

/// The avatar's initials style: `text-xs font-medium text-muted-foreground`.
fn avatar_style(color: Color) -> TextStyle {
    TextStyle {
        family: sans_family(),
        weight: FontWeight::MEDIUM,
        size: style::TEXT_XS as f32,
        color,
        ..TextStyle::default()
    }
}

/// A declarative beUI message row. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust::text;
/// use frust_beui::agents::message::{MessageFrom, message};
///
/// let row = message::<(), _>(MessageFrom::Assistant, text("on it"))
///     .avatar("AI")
///     .name("Assistant")
///     .timestamp("09:41")
///     .animate_in(true);
/// ```
pub struct MessageView<State: 'static> {
    from: MessageFrom,
    content: AnyView<State>,
    avatar: Option<String>,
    avatar_placeholder: bool,
    name: Option<String>,
    timestamp: Option<String>,
    footer: Option<String>,
    animate_in: bool,
}

/// Create a message row from `from` wrapping `content`, with no avatar, no
/// metadata and no entrance.
pub fn message<State: 'static, V: View<State>>(
    from: MessageFrom,
    content: V,
) -> MessageView<State> {
    MessageView {
        from,
        content: any(content),
        avatar: None,
        avatar_placeholder: false,
        name: None,
        timestamp: None,
        footer: None,
        animate_in: false,
    }
}

impl<State: 'static> MessageView<State> {
    /// Show `initials` in the avatar circle (upstream's `MessageAvatar`
    /// children).
    pub fn avatar(mut self, initials: impl Into<String>) -> Self {
        self.avatar = Some(initials.into());
        self.avatar_placeholder = false;
        self
    }

    /// Reserve the avatar's space without painting one (`placeholder`), so a
    /// grouped follow-up row stays aligned with the row above it.
    pub fn avatar_placeholder(mut self, placeholder: bool) -> Self {
        self.avatar_placeholder = placeholder;
        if placeholder {
            self.avatar = None;
        }
        self
    }

    /// Name the sender in the header row.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Stamp the header row with a time.
    pub fn timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    /// Put `footer` below the content (upstream's `MessageFooter`) — a delivery
    /// state, an edit marker.
    pub fn footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    /// Play the trailing-edge pop-up once when this row mounts (`animateIn`).
    pub fn animate_in(mut self, animate_in: bool) -> Self {
        self.animate_in = animate_in;
        self
    }
}

/// The retained widget for a [`MessageView`].
pub struct MessageWidget {
    from: MessageFrom,
    content: ChildPod,
    /// The avatar's initials, shaped once; `None` for a placeholder or no
    /// avatar at all.
    avatar: Option<Label>,
    /// Whether the avatar's box is reserved even with nothing to paint in it.
    avatar_slot: bool,
    name: Option<Label>,
    timestamp: Option<Label>,
    footer: Option<Label>,
    animate_in: bool,
    /// `0.0` un-entered .. `1.0` settled: opacity, rise and scale together.
    entrance: Lane,
    /// Whether the entrance has been staged onto the lane yet.
    staged: bool,
    /// The avatar circle's box, resolved by layout.
    avatar_box: Option<Point>,
    /// Where the header baseline row sits, resolved by layout.
    header_at: Option<(Point, Option<Point>)>,
    /// Where the footer sits, resolved by layout.
    footer_at: Option<Point>,
    /// The row's own box, resolved by layout — the entrance's pivot needs it.
    row: Size,
}

impl MessageWidget {
    /// Who sent this message.
    pub fn from(&self) -> MessageFrom {
        self.from
    }

    /// Whether the avatar's box is reserved (painted or placeholder).
    pub fn has_avatar_slot(&self) -> bool {
        self.avatar_slot
    }

    /// How far through its entrance the row is; `1.0` for a row that never had
    /// one.
    pub fn entrance_progress(&self) -> f64 {
        self.entrance.value()
    }

    /// The content slot's box inside the row, as of the last layout.
    pub fn content_origin(&self) -> Point {
        self.content.origin()
    }

    /// Stage the entrance the first time a clock is available; a row built
    /// without `animateIn` — or under `reduce_motion` — lands immediately.
    fn stage(&mut self, reduce_motion: bool) {
        if self.staged {
            return;
        }
        self.staged = true;
        self.entrance.retarget(1.0);
        if !self.animate_in || reduce_motion {
            self.entrance.snap();
        }
    }

    /// The width the avatar column takes, gap included — zero when no slot is
    /// reserved.
    fn avatar_column(&self) -> f64 {
        if self.avatar_slot {
            MESSAGE_AVATAR_SIZE + MESSAGE_ROW_GAP
        } else {
            0.0
        }
    }
}

impl<State: 'static> View<State> for MessageView<State> {
    type Element = MessageWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MessageWidget {
        let start = if self.animate_in { 0.0 } else { 1.0 };
        MessageWidget {
            from: self.from,
            content: build_child(&self.content, ctx),
            avatar: self.avatar.as_ref().map(Label::new),
            avatar_slot: self.avatar.is_some() || self.avatar_placeholder,
            name: self.name.as_ref().map(Label::new),
            timestamp: self.timestamp.as_ref().map(Label::new),
            footer: self.footer.as_ref().map(Label::new),
            animate_in: self.animate_in,
            entrance: Lane::at_rest(Ramp::spring(SPRING_PRESS), start),
            staged: false,
            avatar_box: None,
            header_at: None,
            footer_at: None,
            row: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MessageWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.content, &self.content, &mut element.content, ctx);
        if element.from != self.from {
            element.from = self.from;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        // Each metadata run re-shapes only when its own text actually changed —
        // `Label::set_content` is the diff, and a run that appears or
        // disappears changes the column's geometry.
        let slot = self.avatar.is_some() || self.avatar_placeholder;
        if element.avatar_slot != slot {
            element.avatar_slot = slot;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        flags |= reconcile(&mut element.avatar, self.avatar.as_deref());
        flags |= reconcile(&mut element.name, self.name.as_deref());
        flags |= reconcile(&mut element.timestamp, self.timestamp.as_deref());
        flags |= reconcile(&mut element.footer, self.footer.as_deref());

        element.animate_in = self.animate_in;
        flags
    }

    fn teardown(&self, element: &mut MessageWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.content, &mut element.content, ctx);
    }
}

/// Reconcile one optional shaped run against its incoming text, returning the
/// flags the change owes. A run whose text is unchanged re-shapes nothing.
fn reconcile(run: &mut Option<Label>, next: Option<&str>) -> ChangeFlags {
    match (run.as_mut(), next) {
        (Some(existing), Some(text)) => {
            if existing.set_content(text) {
                ChangeFlags::LAYOUT | ChangeFlags::PAINT
            } else {
                ChangeFlags::NONE
            }
        }
        (None, None) => ChangeFlags::NONE,
        (_, next) => {
            *run = next.map(Label::new);
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        }
    }
}

impl Widget for MessageWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let row_width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let paint = MessagePaint::resolve(Theme::from_layout_ctx(ctx));
        let meta = meta_style(paint.meta);

        // Shape the metadata first: the column's height is the sum of the rows
        // it actually carries.
        let name = self.name.as_mut().map(|run| run.layout(ctx, &meta));
        let timestamp = self.timestamp.as_mut().map(|run| run.layout(ctx, &meta));
        let footer = self.footer.as_mut().map(|run| run.layout(ctx, &meta));
        if let Some(avatar) = self.avatar.as_mut() {
            avatar.layout(ctx, &avatar_style(paint.meta));
        }

        let column_x = if self.from.is_mirrored() {
            0.0
        } else {
            self.avatar_column()
        };
        let column_width = (row_width - self.avatar_column()).max(0.0);
        let content_size = self.content.layout_child(
            ctx,
            &BoxConstraints::new(Size::new(0.0, 0.0), Size::new(column_width, f64::INFINITY)),
        );

        // The header is one line holding whichever of name/timestamp exist.
        let header_height = match (name, timestamp) {
            (None, None) => 0.0,
            (a, b) => a.map_or(0.0, |s| s.height).max(b.map_or(0.0, |s| s.height)),
        };
        let footer_height = footer.map(|size| size.height.max(MESSAGE_FOOTER_MIN_HEIGHT));

        let mut y = 0.0_f64;
        // `px-1` on the metadata rows, mirrored with the column.
        let meta_inset = MESSAGE_META_PADDING_X;
        if header_height > 0.0 {
            let width = |size: Option<Size>| size.map_or(0.0, |s| s.width);
            let name_w = width(self.name.as_ref().map(Label::size));
            let stamp_w = width(self.timestamp.as_ref().map(Label::size));
            let gap = if name_w > 0.0 && stamp_w > 0.0 {
                META_GAP
            } else {
                0.0
            };
            let total = name_w + gap + stamp_w;
            // `justify-end` for a sent row, `justify-start` otherwise.
            let start_x = if self.from.is_mirrored() {
                column_x + (column_width - total - meta_inset).max(0.0)
            } else {
                column_x + meta_inset
            };
            let name_at = self.name.is_some().then_some(Point::new(start_x, y));
            let stamp_at = self
                .timestamp
                .is_some()
                .then_some(Point::new(start_x + name_w + gap, y));
            self.header_at = Some((name_at.unwrap_or(Point::new(start_x, y)), stamp_at));
            if name_at.is_none() {
                // Only a timestamp: it starts where the header does.
                self.header_at = Some((Point::new(start_x, y), None));
            }
            y += header_height + MESSAGE_COLUMN_GAP;
        } else {
            self.header_at = None;
        }

        // The content column stretches to the column's full width, so a bubble
        // inside it does its own start/end alignment.
        self.content.set_origin(Point::new(column_x, y));
        y += content_size.height;

        if let Some(height) = footer_height {
            y += MESSAGE_COLUMN_GAP;
            let footer_w = self.footer.as_ref().map_or(0.0, |run| run.size().width);
            let x = if self.from.is_mirrored() {
                column_x + (column_width - footer_w - meta_inset).max(0.0)
            } else {
                column_x + meta_inset
            };
            self.footer_at = Some(Point::new(x, y));
            y += height;
        } else {
            self.footer_at = None;
        }

        // `items-start`: the avatar sits at the top of the row, on the side the
        // sender's direction puts it.
        self.avatar_box = self.avatar_slot.then(|| {
            let x = if self.from.is_mirrored() {
                (row_width - MESSAGE_AVATAR_SIZE).max(0.0)
            } else {
                0.0
            };
            Point::new(x, 0.0)
        });

        self.row = Size::new(row_width, y.max(content_size.height));
        bc.constrain(self.row)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let (reduce_motion, paint) = {
            let theme = Theme::from_paint_ctx(ctx);
            (
                theme.is_some_and(|t| t.motion.reduce_motion),
                MessagePaint::resolve(theme),
            )
        };
        self.stage(reduce_motion);

        let now = ctx.frame_time();
        let owes_frame = self.entrance.advance(now);
        let progress = self.entrance.value();
        let alpha = progress.clamp(0.0, 1.0) as f32;
        let origin = ctx.origin();

        // The pop-up: rise, scale and fade together, anchored to the row's own
        // bottom corner on the side the message came from.
        let entering = progress < 1.0;
        if entering {
            let pivot_x = origin.x
                + if self.from.is_mirrored() {
                    self.row.width
                } else {
                    0.0
                };
            let pivot_y = origin.y + self.row.height;
            let scale = MESSAGE_POP_SCALE + (1.0 - MESSAGE_POP_SCALE) * progress;
            let rise = MESSAGE_POP_RISE * (1.0 - progress);
            scene.push_transform(
                Affine::translate((0.0, rise))
                    * Affine::translate((pivot_x, pivot_y))
                    * Affine::scale(scale)
                    * Affine::translate((-pivot_x, -pivot_y)),
            );
            scene.push_layer(origin, self.row, alpha);
        }

        if let Some(at) = self.avatar_box {
            let at = Point::new(origin.x + at.x, origin.y + at.y);
            let box_size = Size::new(MESSAGE_AVATAR_SIZE, MESSAGE_AVATAR_SIZE);
            // A placeholder is `invisible`: it holds the space and paints
            // nothing, which is exactly what keeps a grouped follow-up aligned.
            if let Some(avatar) = self.avatar.as_ref() {
                scene.fill_rounded_rect(at, box_size, MESSAGE_AVATAR_SIZE / 2.0, paint.avatar);
                let text = avatar.size();
                avatar.paint(
                    Point::new(
                        at.x + (MESSAGE_AVATAR_SIZE - text.width) / 2.0,
                        at.y + (MESSAGE_AVATAR_SIZE - text.height) / 2.0,
                    ),
                    scene,
                );
            }
        }

        if let Some((name_at, stamp_at)) = self.header_at {
            if let Some(name) = self.name.as_ref() {
                name.paint(
                    Point::new(origin.x + name_at.x, origin.y + name_at.y),
                    scene,
                );
            }
            let stamp_at = stamp_at.unwrap_or(name_at);
            if let Some(stamp) = self.timestamp.as_ref() {
                stamp.paint(
                    Point::new(origin.x + stamp_at.x, origin.y + stamp_at.y),
                    scene,
                );
            }
        }

        self.content.paint_child(ctx, scene);

        if let Some(at) = self.footer_at
            && let Some(footer) = self.footer.as_ref()
        {
            footer.paint(Point::new(origin.x + at.x, origin.y + at.y), scene);
        }

        if entering {
            scene.pop_layer();
            scene.pop_transform();
        }
        if owes_frame {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.from.aria_label();
        ctx.push_container(
            Role::Group,
            |node| node.set_label(label),
            |ctx| self.content.semantics_child(ctx),
        );
    }

    visit_children!(content);
}

// ---- The marker ------------------------------------------------------------

/// A declarative beUI conversation marker — upstream's `MessageMarker`: the
/// centred pill a transcript uses for "Today", "New messages", "Model
/// switched".
pub struct MessageMarkerView {
    text: String,
}

/// Create a centred marker pill carrying `text`.
///
/// # Example
///
/// ```
/// use frust_beui::agents::message::message_marker;
///
/// let divider = message_marker("Today");
/// ```
pub fn message_marker(text: impl Into<String>) -> MessageMarkerView {
    MessageMarkerView { text: text.into() }
}

/// The retained widget for a [`MessageMarkerView`].
pub struct MessageMarkerWidget {
    label: Label,
    /// The pill's box, resolved by layout.
    pill: Size,
    /// The pill's leading edge inside the row, resolved by layout.
    pill_x: f64,
}

impl MessageMarkerWidget {
    /// The pill's box as of the last layout.
    pub fn pill_size(&self) -> Size {
        self.pill
    }

    /// The pill's leading edge inside the row, as of the last layout.
    pub fn pill_x(&self) -> f64 {
        self.pill_x
    }
}

impl<State: 'static> View<State> for MessageMarkerView {
    type Element = MessageMarkerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MessageMarkerWidget {
        MessageMarkerWidget {
            label: Label::new(&self.text),
            pill: Size::ZERO,
            pill_x: 0.0,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut MessageMarkerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if element.label.set_content(&self.text) {
            ChangeFlags::LAYOUT | ChangeFlags::PAINT
        } else {
            ChangeFlags::NONE
        }
    }

    fn teardown(&self, _element: &mut MessageMarkerWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for MessageMarkerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let row_width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        let ink = MessagePaint::resolve(Theme::from_layout_ctx(ctx)).meta;
        let text = self.label.layout(
            ctx,
            &TextStyle {
                size: style::TEXT_XS as f32,
                ..meta_style(ink)
            },
        );
        let cap = row_width * MESSAGE_MARKER_MAX_WIDTH_FRACTION;
        let pill_width = (text.width + MESSAGE_MARKER_PADDING_X * 2.0).min(cap.max(0.0));
        let pill_height = text.height + MESSAGE_MARKER_PADDING_Y * 2.0;
        self.pill = Size::new(pill_width, pill_height);
        // `mx-auto w-fit`.
        self.pill_x = ((row_width - pill_width) / 2.0).max(0.0);
        bc.constrain(Size::new(row_width, pill_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let paint = MessagePaint::resolve(Theme::from_paint_ctx(ctx));
        let origin = ctx.origin();
        let at = Point::new(origin.x + self.pill_x, origin.y);
        scene.fill_rounded_rect(
            at,
            self.pill,
            style::resolve_radius(style::RADIUS_FULL, self.pill.width, self.pill.height),
            scale_alpha(paint.avatar, MARKER_FILL_ALPHA),
        );
        let text = self.label.size();
        self.label.paint(
            Point::new(
                at.x + (self.pill.width - text.width) / 2.0,
                at.y + MESSAGE_MARKER_PADDING_Y,
            ),
            scene,
        );
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Status, |_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::{FrameTime, text};
    use std::any::Any;

    /// The row every test lays itself into.
    const ROW: Size = Size::new(320.0, 200.0);

    /// Records the rounded-rect fills, transforms and alpha layers a row paints.
    #[derive(Default)]
    struct Recorder {
        rounded: Vec<(Point, Size, Color)>,
        transforms: usize,
        layers: Vec<f32>,
        glyph_runs: usize,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, color: Color) {
            self.rounded.push((origin, size, color));
        }
        fn push_transform(&mut self, _transform: Affine) {
            self.transforms += 1;
        }
        fn push_layer(&mut self, _origin: Point, _size: Size, alpha: f32) {
            self.layers.push(alpha);
        }
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {
            self.glyph_runs += 1;
        }
    }

    fn laid_out(view: &MessageView<()>) -> MessageWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut BuildCtx::new(&mut next_id));
        relayout(&mut widget);
        widget
    }

    fn relayout(widget: &mut MessageWidget) {
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(ROW));
    }

    fn painted(widget: &mut MessageWidget, ms: u64, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, ROW, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// `flex-row-reverse`: a sent row puts its avatar on the trailing edge and
    /// its content column on the leading one; a received row is the mirror.
    #[test]
    fn a_sent_row_mirrors_the_avatar_and_the_content_column() {
        let received = laid_out(&message::<(), _>(MessageFrom::Assistant, text("hi")).avatar("AI"));
        assert_eq!(
            received.content_origin().x,
            MESSAGE_AVATAR_SIZE + MESSAGE_ROW_GAP
        );

        let sent = laid_out(&message::<(), _>(MessageFrom::User, text("hi")).avatar("ED"));
        assert_eq!(sent.content_origin().x, 0.0, "the column leads a sent row");
        assert!(sent.from().is_mirrored());
        assert!(!received.from().is_mirrored());
    }

    /// A placeholder reserves the avatar's box without painting a circle —
    /// what keeps a grouped follow-up row aligned with the row above it.
    #[test]
    fn a_placeholder_avatar_reserves_its_box_and_paints_nothing() {
        let mut with_avatar =
            laid_out(&message::<(), _>(MessageFrom::Assistant, text("hi")).avatar("AI"));
        let (rec, _) = painted(&mut with_avatar, 0, None);
        assert_eq!(rec.rounded.len(), 1, "the avatar circle");

        let mut placeholder = laid_out(
            &message::<(), _>(MessageFrom::Assistant, text("hi")).avatar_placeholder(true),
        );
        let (rec, _) = painted(&mut placeholder, 0, None);
        assert!(placeholder.has_avatar_slot(), "the box is still reserved");
        assert_eq!(
            placeholder.content_origin().x,
            with_avatar.content_origin().x,
            "the column stays aligned"
        );
        assert!(rec.rounded.is_empty(), "`invisible`: nothing is painted");

        let none = laid_out(&message::<(), _>(MessageFrom::Assistant, text("hi")));
        assert!(!none.has_avatar_slot());
        assert_eq!(none.content_origin().x, 0.0);
    }

    /// The metadata rows stack above and below the content at `gap-1.5`, and a
    /// row carrying neither reserves nothing for them.
    #[test]
    fn the_metadata_rows_stack_around_the_content() {
        let bare = laid_out(&message::<(), _>(MessageFrom::Assistant, text("hi")));
        assert_eq!(bare.content_origin().y, 0.0, "no header, no offset");

        let full = laid_out(
            &message::<(), _>(MessageFrom::Assistant, text("hi"))
                .name("Assistant")
                .timestamp("09:41")
                .footer("Delivered"),
        );
        assert!(
            full.content_origin().y > MESSAGE_COLUMN_GAP,
            "the header pushes the content down: {}",
            full.content_origin().y
        );
    }

    /// A row without `animateIn` is settled on its first paint, owes no frame,
    /// and composites through no transform or layer.
    #[test]
    fn a_row_without_animate_in_paints_settled() {
        let mut widget = laid_out(&message::<(), _>(MessageFrom::User, text("hi")));
        let (rec, needs_frame) = painted(&mut widget, 0, None);
        assert!(!needs_frame);
        assert_eq!(widget.entrance_progress(), 1.0);
        assert_eq!(rec.transforms, 0);
        assert!(rec.layers.is_empty());
    }

    /// `animateIn` runs the pop-up: a partial alpha layer and a transform while
    /// in flight, neither once it settles.
    #[test]
    fn animate_in_pops_the_row_up_and_then_stops() {
        let mut widget =
            laid_out(&message::<(), _>(MessageFrom::User, text("hi")).animate_in(true));
        let (rec, needs_frame) = painted(&mut widget, 0, None);
        assert!(needs_frame);
        assert_eq!(rec.transforms, 1, "one entrance transform");
        assert_eq!(rec.layers.len(), 1);
        assert!(rec.layers[0] < 1.0);

        let (_, _) = painted(&mut widget, 40, None);
        let mid = widget.entrance_progress();
        assert!(mid > 0.0 && mid < 1.0, "mid-flight: {mid}");

        let (rec, needs_frame) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame);
        assert_eq!(widget.entrance_progress(), 1.0);
        assert_eq!(rec.transforms, 0, "a settled row composites plainly");
    }

    /// `reduce_motion` lands the pop-up on the first paint.
    #[test]
    fn reduce_motion_lands_the_row_immediately() {
        let theme = reduced();
        let mut widget =
            laid_out(&message::<(), _>(MessageFrom::User, text("hi")).animate_in(true));
        let (rec, needs_frame) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(widget.entrance_progress(), 1.0);
        assert_eq!(rec.transforms, 0);
    }

    /// A rebuild re-shapes a metadata run only when its own text changed, and
    /// a run appearing or disappearing relayouts.
    #[test]
    fn a_rebuild_reshapes_only_the_metadata_that_changed() {
        let base = message::<(), _>(MessageFrom::Assistant, text("hi")).name("Assistant");
        let mut widget = laid_out(&base);
        let mut next_id = 0u64;

        let flags =
            View::<()>::rebuild(&base, &base, &mut widget, &mut BuildCtx::new(&mut next_id));
        assert_eq!(flags, ChangeFlags::NONE, "a redundant rebuild is inert");

        let renamed = message::<(), _>(MessageFrom::Assistant, text("hi")).name("Claude");
        let flags = View::<()>::rebuild(
            &renamed,
            &base,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));

        let stamped = message::<(), _>(MessageFrom::Assistant, text("hi"))
            .name("Claude")
            .timestamp("09:41");
        let flags = View::<()>::rebuild(
            &stamped,
            &renamed,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert!(flags.contains(ChangeFlags::LAYOUT), "a new run is geometry");
    }

    /// A sender swap is geometry: the whole row mirrors.
    #[test]
    fn a_sender_swap_relayouts() {
        let assistant = message::<(), _>(MessageFrom::Assistant, text("hi")).avatar("AI");
        let user = message::<(), _>(MessageFrom::User, text("hi")).avatar("AI");
        let mut widget = laid_out(&assistant);
        let mut next_id = 0u64;
        let flags = View::<()>::rebuild(
            &user,
            &assistant,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert!(flags.contains(ChangeFlags::LAYOUT));
        relayout(&mut widget);
        assert_eq!(widget.content_origin().x, 0.0);
    }

    /// Both senders publish the accessible name upstream gives the row.
    #[test]
    fn every_sender_publishes_its_aria_label() {
        assert_eq!(MessageFrom::ALL.len(), 2);
        assert_eq!(MessageFrom::User.aria_label(), "user message");
        assert_eq!(MessageFrom::Assistant.aria_label(), "assistant message");
    }

    /// The marker is a centred pill capped at 88% of the row.
    #[test]
    fn the_marker_centres_its_pill_and_caps_its_width() {
        let view = message_marker("Today");
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(ROW));

        assert!(widget.pill_size().width > 0.0);
        assert!(widget.pill_size().width <= ROW.width * MESSAGE_MARKER_MAX_WIDTH_FRACTION + 1e-6);
        assert!(
            (widget.pill_x() * 2.0 + widget.pill_size().width - ROW.width).abs() < 1e-6,
            "centred"
        );

        let mut ctx = PaintCtx::for_test(Point::ORIGIN, ROW, FrameTime::ZERO);
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        assert_eq!(recorder.rounded.len(), 1, "one pill");
        assert!(recorder.glyph_runs > 0, "the marker's own text");
    }

    /// A long marker caps rather than overflowing its row.
    #[test]
    fn a_long_marker_caps_at_88_percent() {
        let view = message_marker(
            "a marker long enough that its natural width would overflow the row it sits in",
        );
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(&view, &mut BuildCtx::new(&mut next_id));
        let mut text_ctx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut text_ctx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::loose(ROW));
        assert!(
            widget.pill_size().width <= ROW.width * MESSAGE_MARKER_MAX_WIDTH_FRACTION + 1e-6,
            "capped: {}",
            widget.pill_size().width
        );
    }
}
