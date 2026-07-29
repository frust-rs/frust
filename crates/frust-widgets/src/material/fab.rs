//! The M3 `FloatingActionButton`: regular (56dp) / small (40dp, not
//! deprecated) / large (96dp) container sizes plus an extended variant, with
//! size-scaled icon dimensions (24/24/32dp) and the shared
//! [`super::state_layer`] interaction overlay.
//!
//! [`fab`] produces an icon-only FAB (a fixed square, painted centered icon);
//! [`extended_fab`] produces the pill-shaped extended variant (56dp fixed
//! height, a visible text label, and an optional leading icon). Both fire a
//! plain `on_press` callback on release inside their bounds — mirroring
//! [`crate::Button`]'s fire-on-up-inside contract, not a controlled component
//! (a FAB has no reportable/confirmable value, just an action).
//!
//! # Colors
//!
//! Container/content roles are `primaryContainer`/`onPrimaryContainer`,
//! resolved via [`resolve_colors`]. The extended FAB's visible label is
//! tagged [`crate::text::ThemeTextColor::OnPrimaryContainer`] (a text.rs role
//! addition mirroring the `OnSurfaceVariant` role — see
//! `docs/CODE_STANDARDS.md`'s explicit/theme/fallback precedence).
//!
//! # Elevation
//!
//! A resting FAB paints at M3 elevation level 3 (6dp) via
//! `Theme::elevation.level3` and [`frust_core::PaintScene::draw_shadow`].
//! `draw_shadow`'s primitive has no directional-offset parameter, so this
//! module applies [`frust_theme::ShadowSpec::y_offset`] itself by
//! translating the shadow rect's origin down before painting it — the first
//! widget in the catalog to wire `Elevation` into a real paint call.
//!
//! # Shape
//!
//! Per-size corner radii are M3-published for three of the four sizes: small
//! 12dp (`shape.medium`), regular 16dp (`shape.large`), large 28dp
//! (`shape.extra_large`) — all from the standard M3 FAB spec
//! (m3.material.io/components/floating-action-button/specs). The optional
//! medium size (Expressive-only) has no independently-verified published
//! shape spec; its 20dp radius (`shape.large_increased`) is a **documented v1
//! approximation** following the established small→regular→large scaling
//! pattern, not a fabricated guess from nothing — flagged per
//! `docs/CODE_STANDARDS.md`'s "platform value with no published spec"
//! convention.
//!
//! Extended-FAB padding/gap constants ([`EXTENDED_PAD_ICON_SIDE`] etc.)
//! likewise have no individually published spec; they are a documented v1
//! approximation of the common M3 extended-FAB spacing, in the same spirit as
//! [`super::navbar`]'s hardcoded label type-scale constants.

use std::rc::Rc;

use frust_core::accesskit::{Action, Role};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, View, Widget,
};
use frust_theme::{ShapeScale, Theme};
use kurbo::{Point, Rect, Size};
use peniko::Color;

use super::state_layer::StateLayer;
use crate::text::{self, ThemeTextColor};

/// Container size for the small FAB, in logical px (androidx
/// `FabSmallTokens.ContainerWidth` — not deprecated).
const SMALL_CONTAINER: f64 = 40.0;
/// Container size for the regular FAB, in logical px (androidx
/// `FabBaselineTokens.ContainerWidth`).
const REGULAR_CONTAINER: f64 = 56.0;
/// Container size for the optional medium FAB (Expressive addition), in
/// logical px (androidx `FabMediumTokens.ContainerWidth`).
const MEDIUM_CONTAINER: f64 = 80.0;
/// Container size for the large FAB, in logical px (androidx
/// `FabLargeTokens.ContainerWidth`).
const LARGE_CONTAINER: f64 = 96.0;

/// Icon size for the small FAB, in logical px (small = 24dp).
const SMALL_ICON: f64 = 24.0;
/// Icon size for the regular FAB, in logical px (regular = 24dp).
const REGULAR_ICON: f64 = 24.0;
/// Icon size for the optional medium FAB, in logical px (medium = 28dp).
const MEDIUM_ICON: f64 = 28.0;
/// Icon size for the large FAB, in logical px (large = 32dp).
const LARGE_ICON: f64 = 32.0;
/// Icon size inside an extended FAB, in logical px — fixed regardless of the
/// (ignored) `size` field, matching the M3 extended-FAB spec.
const EXTENDED_ICON: f64 = 24.0;

/// Unthemed-fallback corner radius, small FAB (a theme resolves `shape.medium`
/// — see the module docs).
const SMALL_RADIUS: f64 = 12.0;
/// Unthemed-fallback corner radius, regular/extended FAB (a theme resolves
/// `shape.large` — see the module docs).
const REGULAR_RADIUS: f64 = 16.0;
/// Unthemed-fallback corner radius, medium FAB — **community-approximate**,
/// see the module docs (a theme resolves `shape.large_increased`).
const MEDIUM_RADIUS: f64 = 20.0;
/// Unthemed-fallback corner radius, large FAB (a theme resolves
/// `shape.extra_large`).
const LARGE_RADIUS: f64 = 28.0;

/// Fixed extended-FAB height, in logical px (M3 extended-FAB spec), regardless
/// of the (ignored) `size` field.
const EXTENDED_HEIGHT: f64 = 56.0;
/// Leading padding when an extended FAB has an icon, in logical px —
/// **v1 approximation**, see the module docs.
const EXTENDED_PAD_ICON_SIDE: f64 = 16.0;
/// Trailing padding (label side) when an extended FAB has an icon, in logical
/// px — **v1 approximation**, see the module docs.
const EXTENDED_PAD_LABEL_SIDE: f64 = 20.0;
/// Symmetric padding when an extended FAB has no icon (text-only), in logical
/// px — **v1 approximation**, see the module docs.
const EXTENDED_NO_ICON_PAD: f64 = 20.0;
/// Gap between the icon and the label in an extended FAB, in logical px —
/// **v1 approximation**, see the module docs.
const EXTENDED_GAP: f64 = 8.0;

/// Unthemed-fallback container fill (a theme resolves this from
/// `colors.primary_container`).
const CONTAINER: Color = Color::from_rgb8(0xEA, 0xDD, 0xFF);
/// Unthemed-fallback icon/label fill (a theme resolves this from
/// `colors.on_primary_container`).
const ON_CONTAINER: Color = Color::from_rgb8(0x21, 0x00, 0x5D);

/// Unthemed-fallback shadow y-offset, matching `Elevation::m3().level3`'s
/// `y_offset` exactly (`dp / 2.0 + 1.0` at `dp = 6.0`).
const FALLBACK_SHADOW_Y_OFFSET: f64 = 4.0;
/// Unthemed-fallback shadow blur std-dev, matching `Elevation::m3().level3`.
const FALLBACK_SHADOW_BLUR: f64 = 6.0;
/// Unthemed-fallback shadow color (opaque black at `Elevation::m3().level3`'s
/// `0.3` alpha).
const FALLBACK_SHADOW_COLOR: Color = Color::new([0.0, 0.0, 0.0, 0.3]);

/// The FAB container size tier (small is not deprecated; medium is the
/// optional Expressive addition).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabSize {
    Small,
    Regular,
    Medium,
    Large,
}

impl FabSize {
    fn container(self) -> f64 {
        match self {
            FabSize::Small => SMALL_CONTAINER,
            FabSize::Regular => REGULAR_CONTAINER,
            FabSize::Medium => MEDIUM_CONTAINER,
            FabSize::Large => LARGE_CONTAINER,
        }
    }

    fn icon_size(self) -> f64 {
        match self {
            FabSize::Small => SMALL_ICON,
            FabSize::Regular => REGULAR_ICON,
            FabSize::Medium => MEDIUM_ICON,
            FabSize::Large => LARGE_ICON,
        }
    }

    fn fallback_radius(self) -> f64 {
        match self {
            FabSize::Small => SMALL_RADIUS,
            FabSize::Regular => REGULAR_RADIUS,
            FabSize::Medium => MEDIUM_RADIUS,
            FabSize::Large => LARGE_RADIUS,
        }
    }

    fn themed_radius(self, shape: &ShapeScale) -> f64 {
        match self {
            FabSize::Small => shape.medium,
            FabSize::Regular => shape.large,
            FabSize::Medium => shape.large_increased,
            FabSize::Large => shape.extra_large,
        }
    }
}

/// Return `color` with its alpha channel replaced by `alpha` (mirrors
/// [`super::state_layer`]'s helper of the same shape).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The resolved `(container, on_container)` colors. Themed:
/// `primary_container`/`on_primary_container`. Unthemed: [`CONTAINER`]/
/// [`ON_CONTAINER`] exactly.
fn resolve_colors(theme: Option<&Theme>) -> (Color, Color) {
    match theme {
        Some(theme) => {
            let scheme = theme.scheme();
            (scheme.primary_container, scheme.on_primary_container)
        }
        None => (CONTAINER, ON_CONTAINER),
    }
}

/// The resolved `(blur_std_dev, y_offset, color)` shadow parameters at M3
/// elevation level 3. Themed: `theme.elevation.level3`'s `ShadowSpec`, colored
/// by `colors.shadow` at the spec's `color_alpha`. Unthemed: the
/// [`FALLBACK_SHADOW_BLUR`]/[`FALLBACK_SHADOW_Y_OFFSET`]/
/// [`FALLBACK_SHADOW_COLOR`] constants exactly (the same values
/// `Elevation::m3().level3` produces).
fn resolve_shadow(theme: Option<&Theme>) -> (f64, f64, Color) {
    match theme {
        Some(theme) => {
            let level = theme.elevation.level3;
            let shadow = level.shadow(theme.brightness);
            let color = with_alpha(theme.scheme().shadow, shadow.color_alpha);
            (shadow.blur_std_dev, shadow.y_offset, color)
        }
        None => (
            FALLBACK_SHADOW_BLUR,
            FALLBACK_SHADOW_Y_OFFSET,
            FALLBACK_SHADOW_COLOR,
        ),
    }
}

fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// A view-held, typed press callback (erased on build).
type OnPress<State> = Rc<dyn Fn(&mut State)>;

/// Build the extended FAB's visible label view, themed `OnPrimaryContainer`
/// (see the module docs). Shared by build/rebuild/teardown so the role stays
/// consistent.
fn label_view<State: 'static>(label: String) -> AnyView<State> {
    frust_core::any::<State, _>(text::text(label).themed_role(ThemeTextColor::OnPrimaryContainer))
}

/// A declarative M3 FAB. See the [module docs](self).
pub struct FabView<State: 'static> {
    size: FabSize,
    icon: Option<AnyView<State>>,
    /// Doubles as the extended FAB's visible text (when `extended` is `true`)
    /// and every FAB's accessible name — an icon-only FAB has no visible
    /// text, so [`FabView::label`] is how it gets a screen-reader name.
    label: Option<String>,
    extended: bool,
    on_press: OnPress<State>,
}

/// Create an icon-only, regular-size FAB running `on_press` on release inside
/// its bounds. Chain [`FabView::size`] for small/medium/large, and
/// [`FabView::label`] to attach an accessible name (recommended — an
/// icon-only FAB has no visible text a screen reader can read).
pub fn fab<State: 'static, F: Fn(&mut State) + 'static>(
    icon: AnyView<State>,
    on_press: F,
) -> FabView<State> {
    FabView {
        size: FabSize::Regular,
        icon: Some(icon),
        label: None,
        extended: false,
        on_press: Rc::new(on_press),
    }
}

/// Create an extended FAB labelled `label` (56dp fixed height), running
/// `on_press` on release inside its bounds. Chain [`FabView::icon`] to attach
/// an optional leading icon.
pub fn extended_fab<State: 'static, F: Fn(&mut State) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> FabView<State> {
    FabView {
        size: FabSize::Regular,
        icon: None,
        label: Some(label.into()),
        extended: true,
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> FabView<State> {
    /// Set the container size tier ([`fab`] only — ignored on an
    /// [`extended_fab`], whose height is always [`EXTENDED_HEIGHT`]).
    pub fn size(mut self, size: FabSize) -> Self {
        self.size = size;
        self
    }

    /// Attach a leading icon (usable on [`extended_fab`]; [`fab`] already
    /// requires one at construction).
    pub fn icon(mut self, icon: AnyView<State>) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Attach an accessible label ([`fab`]'s content description). A no-op
    /// override on an [`extended_fab`], whose visible label is already the
    /// accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

/// The retained widget for a [`FabView`].
pub struct FabWidget {
    size: FabSize,
    extended: bool,
    icon: Option<ChildPod>,
    /// The extended FAB's visible label child — `None` for an icon-only FAB
    /// or an extended FAB with no label (shouldn't normally happen, but not
    /// enforced at the type level).
    visible_label: Option<ChildPod>,
    /// The accessible name (see [`FabView::label`]'s docs); always populated
    /// for an extended FAB from its visible text.
    label_text: Option<String>,
    pressed: bool,
    captured: bool,
    state_layer: StateLayer,
    on_press: crate::ErasedCallback,
}

impl<State: 'static> View<State> for FabView<State> {
    type Element = FabWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> FabWidget {
        let icon = self.icon.as_ref().map(|icon| crate::build_child(icon, ctx));
        let visible_label = if self.extended {
            self.label
                .as_ref()
                .map(|label| crate::build_child(&label_view::<State>(label.clone()), ctx))
        } else {
            None
        };
        FabWidget {
            size: self.size,
            extended: self.extended,
            icon,
            visible_label,
            label_text: self.label.clone(),
            pressed: false,
            captured: false,
            state_layer: StateLayer::new(),
            on_press: crate::erase_callback(&self.on_press),
        }
    }

    fn rebuild(&self, prev: &Self, element: &mut FabWidget, ctx: &mut BuildCtx<'_>) -> ChangeFlags {
        element.on_press = crate::erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.extended != self.extended {
            element.extended = self.extended;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }

        match (&prev.icon, &self.icon) {
            (None, None) => {}
            (Some(p), Some(n)) => {
                let pod = element.icon.as_mut().expect("icon pod present");
                flags |= crate::rebuild_child(p, n, pod, ctx);
            }
            (None, Some(n)) => {
                element.icon = Some(crate::build_child(n, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(p), None) => {
                let mut pod = element.icon.take().expect("icon pod present");
                crate::teardown_child(p, &mut pod, ctx);
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }

        if self.extended {
            match (&prev.label, &self.label) {
                (None, None) => {}
                (Some(p), Some(n)) if p == n && prev.extended == self.extended => {}
                (Some(_), Some(n)) => {
                    element.label_text = Some(n.clone());
                    if let Some(pod) = element.visible_label.as_mut() {
                        let prev_view = label_view::<State>(prev.label.clone().unwrap_or_default());
                        let next_view = label_view::<State>(n.clone());
                        flags |= crate::rebuild_child(&prev_view, &next_view, pod, ctx);
                    } else {
                        element.visible_label =
                            Some(crate::build_child(&label_view::<State>(n.clone()), ctx));
                        flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                    }
                }
                (None, Some(n)) => {
                    element.label_text = Some(n.clone());
                    element.visible_label =
                        Some(crate::build_child(&label_view::<State>(n.clone()), ctx));
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
                (Some(_), None) => {
                    element.label_text = None;
                    if let Some(mut pod) = element.visible_label.take() {
                        let prev_view = label_view::<State>(prev.label.clone().unwrap_or_default());
                        crate::teardown_child(&prev_view, &mut pod, ctx);
                    }
                    flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
                }
            }
        } else if prev.label != self.label {
            // Icon-only FAB: the label is accessibility-only, no child to
            // reconcile.
            element.label_text = self.label.clone();
            flags |= ChangeFlags::PAINT;
        }

        flags
    }

    fn teardown(&self, element: &mut FabWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(icon_view), Some(pod)) = (&self.icon, element.icon.as_mut()) {
            crate::teardown_child(icon_view, pod, ctx);
        }
        if self.extended
            && let (Some(label), Some(pod)) = (&self.label, element.visible_label.as_mut())
        {
            let label_view = label_view::<State>(label.clone());
            crate::teardown_child(&label_view, pod, ctx);
        }
    }
}

impl Widget for FabWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        if !self.extended {
            let container = self.size.container();
            if let Some(icon) = self.icon.as_mut() {
                let dim = self.size.icon_size();
                let icon_size = icon.layout_child(ctx, &BoxConstraints::tight(Size::new(dim, dim)));
                icon.set_origin(Point::new(
                    (container - icon_size.width) / 2.0,
                    (container - icon_size.height) / 2.0,
                ));
            }
            return bc.constrain(Size::new(container, container));
        }

        let height = EXTENDED_HEIGHT;
        let has_icon = self.icon.is_some();
        let mut x = if has_icon {
            EXTENDED_PAD_ICON_SIDE
        } else {
            EXTENDED_NO_ICON_PAD
        };
        if let Some(icon) = self.icon.as_mut() {
            let icon_size = icon.layout_child(
                ctx,
                &BoxConstraints::tight(Size::new(EXTENDED_ICON, EXTENDED_ICON)),
            );
            icon.set_origin(Point::new(x, (height - icon_size.height) / 2.0));
            x += icon_size.width + EXTENDED_GAP;
        }
        if let Some(label) = self.visible_label.as_mut() {
            let label_size = label.layout_child(
                ctx,
                &BoxConstraints::loose(Size::new(f64::INFINITY, height)),
            );
            label.set_origin(Point::new(x, (height - label_size.height) / 2.0));
            x += label_size.width;
        }
        x += if has_icon {
            EXTENDED_PAD_LABEL_SIDE
        } else {
            EXTENDED_NO_ICON_PAD
        };
        bc.constrain(Size::new(x, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let (container_color, content_color) = resolve_colors(theme);
        let radius = if self.extended {
            match theme {
                Some(theme) => theme.shape.large,
                None => REGULAR_RADIUS,
            }
        } else {
            match theme {
                Some(theme) => self.size.themed_radius(&theme.shape),
                None => self.size.fallback_radius(),
            }
        };
        let (blur, y_offset, shadow_color) = resolve_shadow(theme);

        let o = ctx.origin();
        let size = ctx.size();
        scene.draw_shadow(
            Point::new(o.x, o.y + y_offset),
            size,
            radius,
            blur,
            shadow_color,
        );
        scene.fill_rounded_rect(o, size, radius, container_color);
        self.state_layer.paint(
            ctx,
            scene,
            Rect::from_origin_size(o, size),
            radius,
            content_color,
        );
        if let Some(icon) = self.icon.as_mut() {
            icon.paint_child(ctx, scene);
        }
        if let Some(label) = self.visible_label.as_mut() {
            label.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        match p.phase {
            PointerPhase::Down => {
                self.pressed = true;
                self.captured = true;
                self.state_layer.set_pressed(true);
                ctx.capture_pointer();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                let inside_now = inside(p.position, ctx.size());
                self.pressed = inside_now;
                self.state_layer.set_pressed(inside_now);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, ctx.size()) {
                    (self.on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.state_layer.set_pressed(false);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            if let Some(label) = &self.label_text {
                node.set_label(label.as_str());
            }
            node.add_action(Action::Click);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf_any;
    use std::any::Any;

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    /// Records each rounded rect's `(origin, size, radius, color)`, each
    /// shadow's `(origin, size, radius, std_dev, color)`.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        shadows: Vec<(Point, Size, f64, f64, Color)>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn draw_shadow(&mut self, o: Point, s: Size, radius: f64, std_dev: f64, color: Color) {
            self.shadows.push((o, s, radius, std_dev, color));
        }
    }

    fn build_icon_fab(size: FabSize) -> FabWidget {
        let view = fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).size(size);
        let mut counter = 0u64;
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn container_sizes_match_ledger_r9() {
        assert_eq!(FabSize::Small.container(), 40.0);
        assert_eq!(FabSize::Regular.container(), 56.0);
        assert_eq!(FabSize::Medium.container(), 80.0);
        assert_eq!(FabSize::Large.container(), 96.0);
    }

    #[test]
    fn icon_sizes_match_ledger_r9() {
        assert_eq!(FabSize::Small.icon_size(), 24.0);
        assert_eq!(FabSize::Regular.icon_size(), 24.0);
        assert_eq!(FabSize::Medium.icon_size(), 28.0);
        assert_eq!(FabSize::Large.icon_size(), 32.0);
    }

    #[test]
    fn layout_is_a_fixed_square_at_the_container_size() {
        let mut w = build_icon_fab(FabSize::Large);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(96.0, 96.0));
        let icon = w.icon.as_ref().unwrap();
        assert_eq!(icon.size(), Size::new(32.0, 32.0));
        // centered
        assert_eq!(icon.origin(), Point::new(32.0, 32.0));
    }

    #[test]
    fn extended_layout_has_fixed_height_and_grows_width() {
        let view: FabView<()> =
            extended_fab("Compose", |_s: &mut ()| {}).icon(leaf_any(24.0, 24.0));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut tcx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size.height, EXTENDED_HEIGHT);
        assert!(size.width > EXTENDED_ICON + EXTENDED_PAD_ICON_SIDE + EXTENDED_PAD_LABEL_SIDE);
    }

    #[test]
    fn unthemed_paint_uses_fallback_container_and_shadow() {
        let mut w = build_icon_fab(FabSize::Regular);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(56.0, 56.0));
        w.paint(&mut pctx, &mut rec);

        assert_eq!(rec.rrects[0].3, CONTAINER);
        assert_eq!(rec.rrects[0].2, REGULAR_RADIUS);
        assert_eq!(rec.shadows.len(), 1, "a resting FAB paints one shadow");
        assert_eq!(rec.shadows[0].3, FALLBACK_SHADOW_BLUR);
        assert_eq!(rec.shadows[0].4, FALLBACK_SHADOW_COLOR);
        assert_eq!(
            rec.shadows[0].0,
            Point::new(0.0, FALLBACK_SHADOW_Y_OFFSET),
            "the shadow origin is offset by y_offset"
        );
    }

    #[test]
    fn themed_paint_resolves_primary_container_tokens() {
        let theme = Theme::m3_baseline();
        let mut w = build_icon_fab(FabSize::Small);
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let mut rec = Recorder::default();
        let mut pctx = PaintCtx::new(Point::ZERO, Size::new(40.0, 40.0)).with_theme(&theme);
        w.paint(&mut pctx, &mut rec);

        let scheme = theme.scheme();
        assert_eq!(rec.rrects[0].3, scheme.primary_container);
        assert_eq!(rec.rrects[0].2, theme.shape.medium);
        assert_eq!(
            rec.shadows[0].4.components[3],
            theme.elevation.level3.shadow(theme.brightness).color_alpha
        );
    }

    /// A minimal `State`-generic icon stand-in (`test_support::leaf`/`leaf_any`
    /// only implement `View<()>`; the interactive tests below need a generic
    /// `State`, so a plain themed [`crate::text::text`] run stands in for the
    /// icon — its content is irrelevant to firing behavior).
    fn icon_stub<State: 'static>() -> AnyView<State> {
        frust_core::any::<State, _>(text::text("i"))
    }

    fn dispatch(w: &mut FabWidget, state: &mut u32, event: &InputEvent, size: Size) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    #[test]
    fn up_inside_fires_on_press() {
        let view: FabView<u32> =
            fab(icon_stub::<u32>(), |s: &mut u32| *s += 1).size(FabSize::Regular);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 28.0, 28.0),
            size,
        );
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 28.0, 28.0), size);
        assert_eq!(state, 1);
    }

    #[test]
    fn up_outside_does_not_fire() {
        let view: FabView<u32> = fab(icon_stub::<u32>(), |s: &mut u32| *s += 1);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 10.0, 10.0),
            size,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Move, 500.0, 500.0),
            size,
        );
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Up, 500.0, 500.0),
            size,
        );
        assert_eq!(state, 0);
    }

    #[test]
    fn cancel_clears_armed_state_without_firing() {
        let view: FabView<u32> = fab(icon_stub::<u32>(), |s: &mut u32| *s += 1);
        let mut counter = 0u64;
        let mut w = View::<u32>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut state = 0u32;
        let size = Size::new(56.0, 56.0);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Down, 10.0, 10.0),
            size,
        );
        assert!(w.captured);
        dispatch(
            &mut w,
            &mut state,
            &ev(PointerPhase::Cancel, 10.0, 10.0),
            size,
        );
        assert!(!w.captured);
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0), size);
        assert_eq!(state, 0);
    }

    #[test]
    fn hover_move_without_down_is_ignored_noop() {
        let mut w = build_icon_fab(FabSize::Regular);
        let mut state = 0u32;
        let state_any: &mut dyn Any = &mut state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(56.0, 56.0));
        let result = w.event(&mut ctx, &ev(PointerPhase::Move, 10.0, 10.0));
        assert!(matches!(result, EventResult::Ignored));
        assert!(!ctx.needs_redraw());
    }

    #[test]
    fn semantics_reports_button_role_and_label() {
        fn logic(_s: &mut ()) -> FabView<()> {
            fab::<(), _>(leaf_any(24.0, 24.0), |_s: &mut ()| {}).label("Compose")
        }
        let mut root: frust_core::RenderRoot<(), FabView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(200.0, 200.0));
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("fab contributes a Role::Button node");
        assert_eq!(node.label(), Some("Compose"));
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn extended_semantics_uses_visible_label_as_accessible_name() {
        fn logic(_s: &mut ()) -> FabView<()> {
            extended_fab::<(), _>("New item", |_s: &mut ()| {})
        }
        let mut root: frust_core::RenderRoot<(), FabView<()>> = frust_core::RenderRoot::new();
        let mut state = ();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(300.0, 200.0), &mut tcx as &mut dyn Any);
        let update = root.semantics();
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("extended fab contributes a Role::Button node");
        assert_eq!(node.label(), Some("New item"));
    }
}
