//! Ports shadcn/ui's **Avatar** (plus its image/fallback/group parts) from
//! `tmp/ui/apps/v4/registry/new-york-v4/ui/avatar.tsx`.
//!
//! [`avatar`] is a single circular slot ([`AvatarSize::Sm`]/`Default`/`Lg` =
//! `size-6`/`size-8`/`size-10` = 24/32/40px) holding either an image
//! ([`AvatarView::image`], cover-fit) or initials
//! ([`AvatarView::fallback`], `bg-muted`/`text-muted-foreground`) — Radix's
//! `Avatar.Image`/`Avatar.Fallback` collapse to one slot here rather than a
//! load-state race, since frust has no async image-load signal to key a
//! fallback off; the caller picks which one to show. [`AvatarView::badge`]
//! adds `AvatarBadge`'s bottom-right `bg-primary`/`ring-background` dot.
//! [`avatar_group`] ports `AvatarGroup`'s `-space-x-2` overlapping row (each
//! member outlined with a `ring-background` halo so overlapping edges read
//! distinctly); [`avatar_group_count`] ports `AvatarGroupCount`'s `+N`
//! circle, reusing the fallback treatment.

use frust::authoring::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, Point, Role, SemanticsCtx, Size, ThemeTextColor,
    ThemeTextType, View, Widget, any,
};
use frust::{Image, ImageFit, ImageSource, Theme, text};
use peniko::Color;

use crate::style;

/// `size-6`/`size-8`/`size-10` — the three avatar diameters, in logical px.
/// `Default` is shadcn's own default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvatarSize {
    /// `size-6` = 24px.
    Sm,
    /// `size-8` = 32px.
    #[default]
    Default,
    /// `size-10` = 40px.
    Lg,
}

impl AvatarSize {
    /// The avatar's diameter, in logical px.
    pub fn px(self) -> f64 {
        match self {
            AvatarSize::Sm => 24.0,
            AvatarSize::Default => 32.0,
            AvatarSize::Lg => 40.0,
        }
    }

    /// `AvatarFallback`'s font size for this avatar size: `text-xs` at `Sm`,
    /// `text-sm` otherwise.
    fn fallback_text_size(self) -> f64 {
        match self {
            AvatarSize::Sm => style::TEXT_XS,
            _ => style::TEXT_SM,
        }
    }

    /// `AvatarBadge`'s diameter for this avatar size: `size-2`/`size-2.5`/`size-3`.
    fn badge_px(self) -> f64 {
        match self {
            AvatarSize::Sm => 8.0,
            AvatarSize::Default => 10.0,
            AvatarSize::Lg => 12.0,
        }
    }
}

/// `ring-2` — the badge's `ring-background` halo width, in logical px.
const BADGE_RING_WIDTH: f64 = 2.0;
/// `-space-x-2` — the overlap between adjacent avatars in an
/// [`avatar_group`], in logical px.
const GROUP_OVERLAP: f64 = 8.0;
/// `ring-2` — an [`avatar_group`] member's separating halo width.
const GROUP_RING_WIDTH: f64 = 2.0;

/// Unthemed-fallback `bg-muted` fallback/badge-count fill.
const FALLBACK_MUTED_FILL: Color = Color::from_rgb8(0xF5, 0xF5, 0xF5);
/// Unthemed-fallback `text-muted-foreground` ink.
const FALLBACK_MUTED_INK: Color = Color::from_rgb8(0x73, 0x73, 0x73);
/// Unthemed-fallback `bg-primary` badge dot.
const FALLBACK_PRIMARY: Color = Color::from_rgb8(0x17, 0x17, 0x17);
/// Unthemed-fallback `ring-background`/page background.
const FALLBACK_BACKGROUND: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);

fn resolve_muted(theme: Option<&Theme>) -> (Color, Color) {
    theme.map_or((FALLBACK_MUTED_FILL, FALLBACK_MUTED_INK), |t| {
        let s = t.scheme();
        (s.surface_container_highest, s.on_surface_variant)
    })
}

fn resolve_badge(theme: Option<&Theme>) -> (Color, Color) {
    theme.map_or((FALLBACK_PRIMARY, FALLBACK_BACKGROUND), |t| {
        let s = t.scheme();
        (s.primary, s.surface)
    })
}

/// What an [`AvatarView`] paints inside its circular clip.
enum AvatarContent {
    /// `AvatarImage`, cover-fit.
    Image(ImageSource),
    /// `AvatarFallback`, shown as initials/text on a muted circle.
    Fallback(String),
}

/// A declarative shadcn avatar. See the [module docs](self).
pub struct AvatarView<State: 'static> {
    size: AvatarSize,
    content: AvatarContent,
    badge: bool,
    _state: std::marker::PhantomData<State>,
}

/// Create a `Default`-size avatar with no content yet — call
/// [`AvatarView::image`] or [`AvatarView::fallback`] to give it one (an
/// avatar with neither paints an empty muted circle, the fallback path with
/// an empty label).
pub fn avatar<State: 'static>() -> AvatarView<State> {
    AvatarView {
        size: AvatarSize::default(),
        content: AvatarContent::Fallback(String::new()),
        badge: false,
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> AvatarView<State> {
    /// Set the avatar's [`AvatarSize`].
    pub fn size(mut self, size: AvatarSize) -> Self {
        self.size = size;
        self
    }

    /// Show `source`, cover-fit and circularly clipped, instead of the
    /// fallback.
    pub fn image(mut self, source: ImageSource) -> Self {
        self.content = AvatarContent::Image(source);
        self
    }

    /// Show `label` (typically initials) on a muted circle.
    pub fn fallback(mut self, label: impl Into<String>) -> Self {
        self.content = AvatarContent::Fallback(label.into());
        self
    }

    /// Add the bottom-right `AvatarBadge` dot.
    pub fn badge(mut self) -> Self {
        self.badge = true;
        self
    }

    fn content_view(&self) -> AnyView<State> {
        match &self.content {
            AvatarContent::Image(source) => any(Image(source.clone()).fit(ImageFit::Cover)),
            AvatarContent::Fallback(label) => any(text(label.clone())
                .size(self.size.fallback_text_size() as f32)
                .themed_family(ThemeTextType::BodyMedium)
                .themed_role(ThemeTextColor::OnSurfaceVariant)),
        }
    }

    fn is_fallback(&self) -> bool {
        matches!(self.content, AvatarContent::Fallback(_))
    }
}

/// The retained widget for an [`AvatarView`].
pub struct AvatarWidget {
    content: ChildPod,
    size: AvatarSize,
    badge: bool,
    is_fallback: bool,
}

impl<State: 'static> View<State> for AvatarView<State> {
    type Element = AvatarWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AvatarWidget {
        AvatarWidget {
            content: frust::authoring::build_child(&self.content_view(), ctx),
            size: self.size,
            badge: self.badge,
            is_fallback: self.is_fallback(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AvatarWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = frust::authoring::rebuild_child(
            &prev.content_view(),
            &self.content_view(),
            &mut element.content,
            ctx,
        );
        element.is_fallback = self.is_fallback();
        if element.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.badge != self.badge {
            element.badge = self.badge;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut AvatarWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.content_view(), &mut element.content, ctx);
    }
}

impl Widget for AvatarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let px = self.size.px();
        let size = bc.constrain(Size::new(px, px));
        if self.is_fallback {
            // `flex items-center justify-center`: the initials keep their
            // natural size and sit in the middle of the circle. A tight
            // constraint here would report the stretched box while the glyph
            // run still painted at its origin — initials in the top-left
            // corner — so the fallback is measured loose (which still bounds
            // its wrap width at the circle's own width) and centred by origin.
            let content = self.content.layout_child(ctx, &BoxConstraints::loose(size));
            self.content.set_origin(Point::new(
                ((size.width - content.width) / 2.0).max(0.0),
                ((size.height - content.height) / 2.0).max(0.0),
            ));
        } else {
            // The image is cover-fit: filling the whole box at the origin is
            // exactly what the circular clip wants.
            self.content.layout_child(ctx, &BoxConstraints::tight(size));
            self.content.set_origin(Point::ZERO);
        }
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let origin = ctx.origin();
        let size = ctx.size();
        let radius = size.width.min(size.height) / 2.0;

        let (fill, _) = resolve_muted(theme);
        let badge_colors = resolve_badge(theme);

        scene.push_clip_rounded(origin, size, radius);
        if self.is_fallback {
            scene.fill_rounded_rect(origin, size, radius, fill);
        }
        self.content.paint_child(ctx, scene);
        scene.pop_clip();

        if self.badge {
            paint_badge(scene, origin, size, self.size, badge_colors);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event_single(&mut self.content, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(Role::Image, |_| {}, |ctx| self.content.semantics_child(ctx));
    }

    frust::authoring::visit_children!(content);
}

/// Paint `AvatarBadge`: a `bg-primary` dot at the bottom-right corner of the
/// `size`x`size` avatar box, ringed by a `ring-background` halo.
fn paint_badge(
    scene: &mut dyn PaintScene,
    avatar_origin: Point,
    avatar_size: Size,
    avatar: AvatarSize,
    colors: (Color, Color),
) {
    let (dot_color, ring_color) = colors;
    let dot_d = avatar.badge_px();
    let dot_r = dot_d / 2.0;
    let center =
        avatar_origin + kurbo::Vec2::new(avatar_size.width - dot_r, avatar_size.height - dot_r);
    let ring_r = dot_r + BADGE_RING_WIDTH;
    scene.fill_rounded_rect(
        Point::new(center.x - ring_r, center.y - ring_r),
        Size::new(ring_r * 2.0, ring_r * 2.0),
        ring_r,
        ring_color,
    );
    scene.fill_rounded_rect(
        Point::new(center.x - dot_r, center.y - dot_r),
        Size::new(dot_d, dot_d),
        dot_r,
        dot_color,
    );
}

/// A declarative `AvatarGroup`: an overlapping row of arbitrary avatar-shaped
/// children, each ringed with a `ring-background` halo so overlapping edges
/// stay distinct.
pub struct AvatarGroupView<State: 'static> {
    children: Vec<AnyView<State>>,
}

/// Create an overlapping avatar row from `children` (typically [`avatar`]
/// views).
pub fn avatar_group<State: 'static>(children: Vec<AnyView<State>>) -> AvatarGroupView<State> {
    AvatarGroupView { children }
}

/// The retained widget for an [`AvatarGroupView`].
pub struct AvatarGroupWidget {
    children: Vec<ChildPod>,
}

impl<State: 'static> View<State> for AvatarGroupView<State> {
    type Element = AvatarGroupWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AvatarGroupWidget {
        AvatarGroupWidget {
            children: self
                .children
                .iter()
                .map(|v| frust::authoring::build_child(v, ctx))
                .collect(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AvatarGroupWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        frust::authoring::rebuild_children(
            &prev.children,
            &self.children,
            &mut element.children,
            ctx,
            |v| v,
            |_| None,
        )
    }

    fn teardown(&self, element: &mut AvatarGroupWidget, ctx: &mut BuildCtx<'_>) {
        for (view, pod) in self.children.iter().zip(element.children.iter_mut()) {
            frust::authoring::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for AvatarGroupWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let loose = BoxConstraints::loose(bc.max());
        let mut x = 0.0_f64;
        let mut height = 0.0_f64;
        for (i, pod) in self.children.iter_mut().enumerate() {
            let s = pod.layout_child(ctx, &loose);
            if i > 0 {
                x -= GROUP_OVERLAP;
            }
            pod.set_origin(Point::new(x, 0.0));
            x += s.width;
            height = height.max(s.height);
        }
        bc.constrain(Size::new(x.max(0.0), height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (_, ring) = resolve_badge(theme);
        let origin = ctx.origin();
        for pod in &mut self.children {
            let child_size = pod.size();
            let radius = child_size.width.min(child_size.height) / 2.0 + GROUP_RING_WIDTH;
            let center = origin
                + pod.origin().to_vec2()
                + kurbo::Vec2::new(child_size.width / 2.0, child_size.height / 2.0);
            scene.fill_rounded_rect(
                Point::new(center.x - radius, center.y - radius),
                Size::new(radius * 2.0, radius * 2.0),
                radius,
                ring,
            );
        }
        // Later (rightmost) members paint over earlier ones, matching the
        // source's natural DOM stacking order.
        for pod in &mut self.children {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        frust::authoring::route_event(&mut self.children, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_container(
            Role::GenericContainer,
            |_| {},
            |ctx| {
                for pod in &self.children {
                    pod.semantics_child(ctx);
                }
            },
        );
    }

    frust::authoring::visit_children!(children);
}

/// `AvatarGroupCount`: a `+N`-style circle in the fallback treatment
/// (`bg-muted`/`text-muted-foreground`), sized like an ordinary avatar.
pub struct AvatarGroupCountView<State: 'static> {
    label: String,
    size: AvatarSize,
    _state: std::marker::PhantomData<State>,
}

/// Create an `AvatarGroupCount` circle labelled `label` (e.g. `"+3"`), at
/// [`AvatarSize::Default`] unless overridden with
/// [`AvatarGroupCountView::size`].
pub fn avatar_group_count<State: 'static>(label: impl Into<String>) -> AvatarGroupCountView<State> {
    AvatarGroupCountView {
        label: label.into(),
        size: AvatarSize::default(),
        _state: std::marker::PhantomData,
    }
}

impl<State: 'static> AvatarGroupCountView<State> {
    /// Set the circle's [`AvatarSize`] — the source derives this from the
    /// enclosing group's own size via a `group-has-data-size` selector, which
    /// has no frust counterpart; a caller names it directly instead.
    pub fn size(mut self, size: AvatarSize) -> Self {
        self.size = size;
        self
    }
}

impl<State: 'static> View<State> for AvatarGroupCountView<State> {
    type Element = <AvatarView<State> as View<State>>::Element;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        avatar::<State>()
            .size(self.size)
            .fallback(self.label.clone())
            .build(ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let prev_view = avatar::<State>()
            .size(prev.size)
            .fallback(prev.label.clone());
        let next_view = avatar::<State>()
            .size(self.size)
            .fallback(self.label.clone());
        View::<State>::rebuild(&next_view, &prev_view, element, ctx)
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        avatar::<State>()
            .size(self.size)
            .fallback(self.label.clone())
            .teardown(element, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::text::TextContext;
    use frust::authoring::{Affine, BezPath, Brush};
    use std::any::Any;

    #[derive(Default)]
    struct Recorder {
        rounded_rects: Vec<(Point, Size, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rounded_rects.push((o, s, radius, color));
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _b: &Brush) {}
        fn push_transform(&mut self, _t: Affine) {}
    }

    fn build<S: 'static>(view: &AvatarView<S>) -> AvatarWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut AvatarWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(200.0, 200.0)))
    }

    fn paint(w: &mut AvatarWidget, size: Size) -> Recorder {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut ctx, &mut rec);
        rec
    }

    #[test]
    fn default_size_is_32px_and_paints_a_muted_fallback_circle() {
        let view: AvatarView<()> = avatar().fallback("AB");
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size, Size::new(32.0, 32.0));
        let rec = paint(&mut w, size);
        assert_eq!(rec.rounded_rects[0].2, 16.0, "the clip-matching radius");
        assert_eq!(rec.rounded_rects[0].3, FALLBACK_MUTED_FILL);
    }

    #[test]
    fn lg_size_is_40px() {
        let view: AvatarView<()> = avatar().size(AvatarSize::Lg).fallback("Z");
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(size, Size::new(40.0, 40.0));
    }

    #[test]
    fn a_badge_paints_a_ring_and_a_dot_beyond_the_fallback_circle() {
        let view: AvatarView<()> = avatar().fallback("AB").badge();
        let mut w = build(&view);
        let size = layout(&mut w);
        let rec = paint(&mut w, size);
        // [0] fallback circle, [1] badge ring, [2] badge dot.
        assert_eq!(rec.rounded_rects.len(), 3);
        assert_eq!(rec.rounded_rects[1].3, FALLBACK_BACKGROUND);
        assert_eq!(rec.rounded_rects[2].3, FALLBACK_PRIMARY);
        assert!(
            rec.rounded_rects[1].2 > rec.rounded_rects[2].2,
            "the ring is wider than the dot"
        );
    }

    #[test]
    fn no_badge_paints_no_dot() {
        let view: AvatarView<()> = avatar().fallback("AB");
        let mut w = build(&view);
        let size = layout(&mut w);
        let rec = paint(&mut w, size);
        assert_eq!(rec.rounded_rects.len(), 1);
    }

    #[test]
    fn fallback_content_is_centred_in_the_circle() {
        let view: AvatarView<()> = avatar().fallback("AB");
        let mut w = build(&view);
        let size = layout(&mut w);
        let content = w.content.size();
        assert!(
            content.width < size.width && content.height < size.height,
            "the initials keep their natural size, not the stretched box"
        );
        assert_eq!(
            w.content.origin(),
            Point::new(
                (size.width - content.width) / 2.0,
                (size.height - content.height) / 2.0
            ),
            "centred on both axes, not pinned to the top-left corner"
        );
        assert_ne!(w.content.origin(), Point::ZERO);
    }

    #[test]
    fn an_image_still_fills_the_whole_circle_from_its_origin() {
        let source = ImageSource::from_rgba8(vec![0u8; 4 * 4 * 4], 4, 4);
        let view: AvatarView<()> = avatar().image(source);
        let mut w = build(&view);
        let size = layout(&mut w);
        assert_eq!(w.content.size(), size, "cover-fit fills the clip");
        assert_eq!(w.content.origin(), Point::ZERO);
    }

    #[test]
    fn group_overlaps_members_by_8px() {
        let view: AvatarGroupView<()> = avatar_group(vec![
            any(avatar::<()>().fallback("A")),
            any(avatar::<()>().fallback("B")),
        ]);
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(300.0, 100.0)));
        assert_eq!(w.children[0].origin().x, 0.0);
        assert_eq!(w.children[1].origin().x, 32.0 - GROUP_OVERLAP);
        assert_eq!(size.width, 32.0 + (32.0 - GROUP_OVERLAP));
    }

    #[test]
    fn group_count_reuses_the_fallback_circle_treatment() {
        let view: AvatarGroupCountView<()> = avatar_group_count("+3");
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(100.0, 100.0)));
        assert_eq!(size, Size::new(32.0, 32.0));
    }

    // ---- Typeface: the fallback initials follow the live theme -----------

    /// The fallback at both text sizes (`Sm` is `text-xs`), and the group
    /// count that reuses it.
    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_fallback_paints_in_the_theme_face() {
        use crate::text::typeface_probe::assert_paints_in_the_theme_face;
        for size in [AvatarSize::Sm, AvatarSize::Default] {
            assert_paints_in_the_theme_face(
                &format!("a {size:?} avatar's fallback"),
                |_: &mut ()| avatar::<()>().size(size).fallback("ED"),
            );
        }
        assert_paints_in_the_theme_face("an avatar group's count", |_: &mut ()| {
            avatar_group_count::<()>("+3")
        });
    }

    #[cfg(feature = "bundled-fonts")]
    #[test]
    fn the_fallback_follows_a_live_theme_swap() {
        use crate::text::typeface_probe::assert_follows_a_live_theme_swap;
        for size in [AvatarSize::Sm, AvatarSize::Default] {
            assert_follows_a_live_theme_swap(
                &format!("a {size:?} avatar's fallback"),
                |_: &mut ()| avatar::<()>().size(size).fallback("ED"),
            );
        }
        assert_follows_a_live_theme_swap("an avatar group's count", |_: &mut ()| {
            avatar_group_count::<()>("+3")
        });
    }
}
