//! Phase 6a exit-criterion demo: the theme + motion gallery (spec §17/§18).
//!
//! Proves the whole 6a design-token stack live in one screen: a Material 3
//! color-role swatch grid, the 15-token type scale rendered as styled text,
//! real gaussian-blurred elevation shadows over `surface`, an app-forced
//! light/dark toggle, and two motion demos — a duration+[`Curve::Emphasized`]
//! tween, and the M3 Expressive `default-spatial`/`default-effects` spring
//! presets side by side (the former visibly overshoots on settle, the latter
//! doesn't). This is the manual visual gate `docs/DEVELOPMENT.md` documents
//! for theme/animation changes; see `cargo run -p gallery`.
//!
//! ## Why this crate depends on `forgekit-core`/`kurbo`/`peniko` directly
//!
//! Every other in-workspace example (`hello`/`counter`/`notes`) depends only
//! on the `forgekit` facade — its curated widget vocabulary (`Text`,
//! `Button`, …) is enough to build their screens. This gallery needs two
//! things no facade widget paints yet: an arbitrary-color filled rect (the
//! swatch grid) and a real blurred drop shadow (the elevation cards), plus a
//! paint-driven animation (the motion boxes) — so [`ColorBoxView`]/
//! [`MotionBoxView`] below are small `View`/`Widget` pairs built directly
//! against `forgekit-core` (spec §6), exactly the way `forgekit-widgets`
//! itself builds every prebuilt widget. This mirrors the facade's own
//! documented "low-level escape hatch" (`forgekit::App::new`, see
//! `crates/forgekit/src/lib.rs`) rather than inventing anything: no new
//! module, no unreleased feature, just the existing `Widget`/`PaintScene`
//! seam used one layer down from the facade. Everything else — layout
//! containers, `Button`, reading the theme via `use_context::<Theme>()` —
//! goes through the facade exactly like every other example.

use std::time::Duration;

use forgekit::{
    AnimationController, AnyView, Button, ColorScheme, Column, Component, Curve, EdgeInsets,
    ElevationLevel, FrameTime, MotionSpring, Padding, Row, ShadowSpec, SizedBox, Spring,
    SpringDesc, SurfaceRole, Theme, any, scroll_view, text, use_context,
};
use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::{Point, Size};
use peniko::Color;

// --- ColorBoxView/Widget: a flat swatch, or (with a shadow) an elevation card ---

/// A themed rectangle: a flat color swatch (`shadow: None`), or — with a
/// [`ShadowSpec`] attached via [`ColorBoxView::shadow`] — a real
/// gaussian-blurred elevation card. See the module docs for why this is a
/// small hand-rolled `View`/`Widget` pair rather than a facade widget.
struct ColorBoxView {
    size: Size,
    fill: Color,
    radius: f64,
    shadow: Option<(ShadowSpec, Color)>,
}

fn color_box(size: Size, fill: Color, radius: f64) -> ColorBoxView {
    ColorBoxView {
        size,
        fill,
        radius,
        shadow: None,
    }
}

impl ColorBoxView {
    /// Attach a real blurred elevation shadow (drawn under the fill, offset
    /// by `spec.y_offset`), turning the swatch into an elevation card.
    fn shadow(mut self, spec: ShadowSpec, color: Color) -> Self {
        self.shadow = Some((spec, color));
        self
    }
}

/// The retained widget for a [`ColorBoxView`]. Childless leaf: fixed size,
/// filled rect, optional shadow.
struct ColorBoxWidget {
    size: Size,
    fill: Color,
    radius: f64,
    shadow: Option<(ShadowSpec, Color)>,
}

impl<State: 'static> View<State> for ColorBoxView {
    type Element = ColorBoxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ColorBoxWidget {
        ColorBoxWidget {
            size: self.size,
            fill: self.fill,
            radius: self.radius,
            shadow: self.shadow,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ColorBoxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.size = self.size;
        element.fill = self.fill;
        element.radius = self.radius;
        element.shadow = self.shadow;
        ChangeFlags::PAINT
    }
}

impl Widget for ColorBoxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        if let Some((spec, color)) = self.shadow {
            let shadow_origin = Point::new(origin.x, origin.y + spec.y_offset);
            scene.draw_shadow(
                shadow_origin,
                size,
                self.radius,
                spec.blur_std_dev,
                color.with_alpha(spec.color_alpha),
            );
        }
        scene.fill_rounded_rect(origin, size, self.radius, self.fill);
    }
}

// --- MotionBoxView/Widget: a puck sliding along a track, frame-clock-driven ---

/// Which motion primitive drives a [`MotionBoxView`]'s puck: a duration +
/// [`Curve`] tween, or a physics [`SpringDesc`] fling.
#[derive(Clone, Copy)]
enum MotionKind {
    Curve(Curve, Duration),
    Spring(SpringDesc),
}

/// Initial fling velocity (value-units/second) for the spring demos — picked
/// so an under-damped preset (M3's `*-spatial` springs) visibly overshoots
/// before settling, and a critically-damped one (`*-effects`) visibly does
/// not.
const FLING_VELOCITY: f64 = 3.0;

/// The rest epsilon [`Spring::is_at_rest`] settles against, mirroring
/// `forgekit-core::anim`'s own (private) `SPRING_REST_EPSILON`.
const SPRING_REST_EPSILON: f64 = 1e-3;

/// The active driver behind a [`MotionBoxWidget`]'s current value.
enum Drive {
    Curve(AnimationController),
    Spring(Spring),
}

/// An animated "puck sliding along a track", driven by the shell frame clock
/// (spec §8) during its own paint — the gallery's motion-page primitive. See
/// the module docs for why this is a hand-rolled `View`/`Widget` pair.
struct MotionBoxView {
    kind: MotionKind,
    trigger: u64,
    track: Size,
    puck: f64,
    track_color: Color,
    puck_color: Color,
}

fn motion_box(
    kind: MotionKind,
    trigger: u64,
    track: Size,
    puck: f64,
    track_color: Color,
    puck_color: Color,
) -> MotionBoxView {
    MotionBoxView {
        kind,
        trigger,
        track,
        puck,
        track_color,
        puck_color,
    }
}

struct MotionBoxWidget {
    kind: MotionKind,
    trigger: u64,
    track: Size,
    puck: f64,
    track_color: Color,
    puck_color: Color,
    drive: Drive,
    last_time: Option<FrameTime>,
    elapsed: f64,
    value: f64,
}

impl MotionBoxWidget {
    /// (Re)start the motion from `value = 0`, seeding a fresh driver so a
    /// "Replay" tap always shows the full motion again.
    fn restart(&mut self) {
        self.last_time = None;
        self.elapsed = 0.0;
        self.value = 0.0;
        self.drive = match self.kind {
            MotionKind::Curve(curve, duration) => {
                let mut controller = AnimationController::new(duration).with_curve(curve);
                controller.forward();
                Drive::Curve(controller)
            }
            MotionKind::Spring(desc) => Drive::Spring(Spring::new(desc, -1.0, FLING_VELOCITY)),
        };
    }

    /// Advance to `now`, updating `self.value`; returns whether it is still
    /// in motion (the caller must keep requesting frames while `true`).
    fn advance(&mut self, now: FrameTime) -> bool {
        match &mut self.drive {
            Drive::Curve(controller) => {
                let animating = controller.advance(now);
                self.value = controller.value();
                animating
            }
            Drive::Spring(spring) => {
                let dt = match self.last_time {
                    Some(last) => now.saturating_sub(last).as_secs_f64(),
                    None => 0.0,
                };
                self.last_time = Some(now);
                self.elapsed += dt;
                if spring.is_at_rest(self.elapsed, SPRING_REST_EPSILON) {
                    self.value = 1.0;
                    false
                } else {
                    // `1.0 +` un-clamped: an under-damped preset's overshoot past
                    // the 1.0 rest position is the whole point (the "bounce" this
                    // demo contrasts against a critically damped preset's
                    // monotonic approach) — clamping it away would hide exactly
                    // what the demo exists to show.
                    self.value = 1.0 + spring.position(self.elapsed);
                    true
                }
            }
        }
    }
}

impl<State: 'static> View<State> for MotionBoxView {
    type Element = MotionBoxWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MotionBoxWidget {
        let mut widget = MotionBoxWidget {
            kind: self.kind,
            trigger: self.trigger,
            track: self.track,
            puck: self.puck,
            track_color: self.track_color,
            puck_color: self.puck_color,
            // Placeholder, immediately replaced by `restart()` below.
            drive: Drive::Curve(AnimationController::new(Duration::from_millis(1))),
            last_time: None,
            elapsed: 0.0,
            value: 0.0,
        };
        widget.restart();
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MotionBoxWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.track = self.track;
        element.puck = self.puck;
        element.track_color = self.track_color;
        element.puck_color = self.puck_color;
        if self.trigger != prev.trigger {
            element.trigger = self.trigger;
            element.restart();
        }
        ChangeFlags::PAINT
    }
}

impl Widget for MotionBoxWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.track)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let animating = self.advance(ctx.frame_time());
        let origin = ctx.origin();
        let size = ctx.size();
        scene.fill_rounded_rect(origin, size, size.height / 2.0, self.track_color);

        let travel = (size.width - self.puck).max(0.0);
        let x = (origin.x + self.value * travel).clamp(
            origin.x - self.puck * 0.2,
            origin.x + size.width - self.puck * 0.8,
        );
        scene.fill_rounded_rect(
            Point::new(x, origin.y),
            Size::new(self.puck, size.height),
            size.height / 2.0,
            self.puck_color,
        );

        if animating {
            ctx.request_frame();
        }
    }
}

// --- The gallery app itself ---

/// The gallery's top-level screen selector — a top-row button toggles it, per
/// the task's "no navigation framework exists yet" scope.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Theme,
    Motion,
}

/// The gallery's retained [`Component::State`] (spec §5.5): which page is
/// showing, the app-forced light/dark toggle, and a replay counter the
/// motion page's boxes watch to restart their animation.
///
/// `dark` is an **app-forced** brightness, deliberately decoupled from the
/// shell's own OS-driven `Theme::brightness` (see `GalleryApp::build`) — the
/// desktop shell doesn't yet expose an app-facing "force this brightness"
/// API, so this demonstrates the documented minimum bar (task 09): toggling
/// it live-repaints the whole page correctly, via a plain `Component::State`
/// flag and `use_context::<Theme>()`'s already-facade-exposed light/dark
/// tables, with no framework change.
pub struct GalleryState {
    page: Page,
    dark: bool,
    replay: u64,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            page: Page::Theme,
            dark: false,
            replay: 0,
        }
    }
}

/// The gallery's root [`Component`] (spec §5.5), bound to all three platforms
/// by [`forgekit::app!`] below.
#[derive(Default)]
pub struct GalleryApp;

impl Component for GalleryApp {
    type State = GalleryState;

    fn init(&self) -> GalleryState {
        GalleryState::default()
    }

    fn build(&self, state: &mut GalleryState) -> AnyView<GalleryState> {
        // App-side theme read (spec §11): resolves the theme the desktop shell
        // `provide_context`s (see `crates/forgekit-shell-desktop`); falls back to
        // the M3 baseline when no context is ambient (a bare test/host — see
        // `tests::build_succeeds_for_both_brightness_states` below).
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        // The app-forced brightness (see `GalleryState` docs) rather than
        // `theme.scheme()`, which instead follows the shell's own OS-driven
        // `theme.brightness` — the two are intentionally decoupled here.
        let scheme = if state.dark { theme.dark } else { theme.light };

        let mut children: Vec<AnyView<GalleryState>> = Vec::new();
        children.push(any(text("ForgeKit Gallery")
            .size(28.0)
            .color(scheme.on_surface)));
        children.push(any(Row(vec![
            any(Button("Theme", |s: &mut GalleryState| s.page = Page::Theme)),
            any(SizedBox(Some(8.0), None)),
            any(Button("Motion", |s: &mut GalleryState| {
                s.page = Page::Motion
            })),
            any(SizedBox(Some(8.0), None)),
            any(Button(
                if state.dark {
                    "Switch to light"
                } else {
                    "Switch to dark"
                },
                |s: &mut GalleryState| s.dark = !s.dark,
            )),
        ])));
        children.push(any(SizedBox(None, Some(16.0))));

        match state.page {
            Page::Theme => children.extend(theme_page(&theme, scheme)),
            Page::Motion => children.extend(motion_page(&theme, scheme, state.replay)),
        }

        any(scroll_view(Column(children)))
    }
}

/// Which `ColorScheme` field a [`SurfaceRole`] names (the M3 static
/// surface-container direction — see `forgekit-theme::elevation`'s module
/// docs).
fn surface_role_color(scheme: ColorScheme, role: SurfaceRole) -> Color {
    match role {
        SurfaceRole::Surface => scheme.surface,
        SurfaceRole::SurfaceContainerLowest => scheme.surface_container_lowest,
        SurfaceRole::SurfaceContainerLow => scheme.surface_container_low,
        SurfaceRole::SurfaceContainer => scheme.surface_container,
        SurfaceRole::SurfaceContainerHigh => scheme.surface_container_high,
        SurfaceRole::SurfaceContainerHighest => scheme.surface_container_highest,
    }
}

/// One labeled swatch cell: a flat [`ColorBoxView`] plus a caption below.
fn swatch_cell(scheme: ColorScheme, color: Color, label: &'static str) -> AnyView<GalleryState> {
    any(Padding(
        EdgeInsets::all(6.0),
        Column(vec![
            any(color_box(Size::new(84.0, 48.0), color, 8.0)),
            any(text(label).size(12.0).color(scheme.on_surface)),
        ]),
    ))
}

/// One elevation card cell: a shadowed [`ColorBoxView`] at the level's mapped
/// surface-container role, plus a caption.
fn elevation_cell(
    theme: &Theme,
    scheme: ColorScheme,
    level: ElevationLevel,
    label: &'static str,
) -> AnyView<GalleryState> {
    let fill = surface_role_color(scheme, level.surface_role);
    any(Padding(
        EdgeInsets::all(6.0),
        Column(vec![
            any(color_box(Size::new(96.0, 64.0), fill, theme.shape.medium)
                .shadow(level.shadow, scheme.shadow)),
            any(text(label).size(12.0).color(scheme.on_surface)),
        ]),
    ))
}

/// The theme page: color swatch grid, 15-token type scale, elevation cards.
fn theme_page(theme: &Theme, scheme: ColorScheme) -> Vec<AnyView<GalleryState>> {
    let mut v: Vec<AnyView<GalleryState>> = Vec::new();

    v.push(any(text("Color roles").size(20.0).color(scheme.on_surface)));
    let swatches = [
        (scheme.primary_container, "primary container"),
        (scheme.secondary_container, "secondary container"),
        (scheme.tertiary_container, "tertiary container"),
        (scheme.error_container, "error container"),
        (scheme.surface, "surface"),
        (scheme.surface_container_low, "surface container low"),
        (scheme.surface_container, "surface container"),
        (scheme.surface_container_high, "surface container high"),
        (
            scheme.surface_container_highest,
            "surface container highest",
        ),
    ];
    for row in swatches.chunks(5) {
        let cells = row
            .iter()
            .map(|(color, label)| swatch_cell(scheme, *color, label))
            .collect();
        v.push(any(Row(cells)));
    }

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(text("Type scale").size(20.0).color(scheme.on_surface)));
    let scale = &theme.type_scale;
    let tokens = [
        ("Display large", scale.display_large.clone()),
        ("Display medium", scale.display_medium.clone()),
        ("Display small", scale.display_small.clone()),
        ("Headline large", scale.headline_large.clone()),
        ("Headline medium", scale.headline_medium.clone()),
        ("Headline small", scale.headline_small.clone()),
        ("Title large", scale.title_large.clone()),
        ("Title medium", scale.title_medium.clone()),
        ("Title small", scale.title_small.clone()),
        ("Body large", scale.body_large.clone()),
        ("Body medium", scale.body_medium.clone()),
        ("Body small", scale.body_small.clone()),
        ("Label large", scale.label_large.clone()),
        ("Label medium", scale.label_medium.clone()),
        ("Label small", scale.label_small.clone()),
    ];
    for (label, style) in tokens {
        v.push(any(text(label).style(style).color(scheme.on_surface)));
    }

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(text("Elevation").size(20.0).color(scheme.on_surface)));
    let elevation = &theme.elevation;
    let cards = vec![
        elevation_cell(theme, scheme, elevation.level1, "level 1"),
        elevation_cell(theme, scheme, elevation.level2, "level 2"),
        elevation_cell(theme, scheme, elevation.level3, "level 3"),
        elevation_cell(theme, scheme, elevation.level4, "level 4"),
    ];
    v.push(any(Row(cards)));

    v
}

/// `forgekit-theme::MotionSpring` (mass implicitly `1.0`, see its module
/// docs) into `forgekit-core::anim`'s `SpringDesc`, the generic physics type
/// `AnimationController::fling`/[`Spring::new`] consume.
fn spring_desc(spring: MotionSpring) -> SpringDesc {
    SpringDesc {
        mass: 1.0,
        stiffness: spring.stiffness,
        damping_ratio: spring.damping_ratio,
    }
}

/// The motion page: a duration+[`Curve::Emphasized`] tween, and the M3
/// Expressive `default-spatial` (bounce) vs `default-effects` (no bounce)
/// spring presets side by side — both restart on the trailing "Replay"
/// button, which bumps `trigger`.
fn motion_page(theme: &Theme, scheme: ColorScheme, trigger: u64) -> Vec<AnyView<GalleryState>> {
    let track = Size::new(320.0, 48.0);
    let mut v: Vec<AnyView<GalleryState>> = Vec::new();

    v.push(any(text("Duration + curve (Emphasized)")
        .size(18.0)
        .color(scheme.on_surface)));
    v.push(any(motion_box(
        MotionKind::Curve(Curve::Emphasized, Duration::from_millis(900)),
        trigger,
        track,
        48.0,
        scheme.surface_container_highest,
        scheme.primary,
    )));

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(text(
        "Spring: default-spatial (bounce) vs default-effects (no bounce)",
    )
    .size(18.0)
    .color(scheme.on_surface)));
    v.push(any(motion_box(
        MotionKind::Spring(spring_desc(theme.motion.default_spatial)),
        trigger,
        track,
        48.0,
        scheme.surface_container_highest,
        scheme.primary,
    )));
    v.push(any(SizedBox(None, Some(8.0))));
    v.push(any(motion_box(
        MotionKind::Spring(spring_desc(theme.motion.default_effects)),
        trigger,
        track,
        48.0,
        scheme.surface_container_highest,
        scheme.tertiary,
    )));

    v.push(any(SizedBox(None, Some(16.0))));
    v.push(any(Button("Replay", |s: &mut GalleryState| {
        s.replay = s.replay.wrapping_add(1);
    })));

    v
}

// The canonical app entry point (spec §5.5): binds `GalleryApp` to all three
// platforms — Android JNI exports, iOS C-ABI exports, and the desktop
// `__forgekit_main` `main.rs` calls. Never hand-edited.
forgekit::app!(GalleryApp);

#[cfg(test)]
mod tests {
    use super::*;

    /// View construction only, no GPU (task 09's acceptance criterion): the
    /// root `Component::build` must succeed for both brightness states and
    /// both pages, without a `ReactiveRuntime`/theme context ambient
    /// (`use_context` gracefully resolves to `None` there, falling back to
    /// `Theme::m3_baseline`).
    #[test]
    fn build_succeeds_for_both_brightness_states() {
        let mut state = GalleryState::default();
        let _ = GalleryApp.build(&mut state);

        state.dark = true;
        let _ = GalleryApp.build(&mut state);

        state.page = Page::Motion;
        let _ = GalleryApp.build(&mut state);
    }
}
