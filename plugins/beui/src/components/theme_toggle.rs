//! Ports beUI's `theme-toggle` component.
//!
//! **Source:** `components/motion/theme-toggle.tsx`, beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01.
//!
//! # What upstream does, and what a port can keep
//!
//! Upstream is two things bolted together: a **button** that swaps a sun for a
//! moon, and a **whole-page repaint** driven by the browser's View Transition
//! API — `startViewTransition` snapshots the document, `next-themes` flips the
//! class, and a `::view-transition-new(root)` rule reveals the new page through
//! a clip-path (a rectangle wipe, a circular expand, or masked slats).
//!
//! The button ports directly. The page repaint does not: frust has no
//! view-transition seam — nothing snapshots the previous frame's window and
//! cross-fades it against the next — and inventing one is a framework feature,
//! not a component. So the **reveal is kept and scoped to the toggle's own
//! box**: [`ThemeToggleVariant`] chooses how the new icon is revealed over the
//! old, and [`ThemeToggleStart`] chooses where the reveal starts from, exactly
//! as upstream's `variant`/`start` pair does — a real, visible choice rather
//! than a prop recorded and ignored. What is lost is the *scale* of the effect:
//! the page behind the toggle simply changes colour.
//!
//! # How the theme actually switches
//!
//! Through [`frust::set_app_theme`] — the same single-call-site mechanism the
//! catalogs' own demo apps use (`examples/glyph-catalog`'s `apply_theme`,
//! `examples/huddle`'s settings use case): a [`ThemeBuilder`](frust::ThemeBuilder)
//! over this catalog's own [`theme()`](crate::theme) with the wanted brightness
//! pinned. The override is process-global and the running shell picks it up on
//! its next frame.
//!
//! That is deliberately `set_app_theme` and not
//! [`set_default_theme`](frust::set_default_theme): the point of the control is
//! that the user's choice *outranks* the platform's, and only the override slot
//! pins brightness. An app that owns its own theming turns the push off with
//! [`ThemeToggleView::drive_app_theme`] and does the work in
//! [`ThemeToggleView::on_toggle`].
//!
//! # Degradations against upstream
//!
//! - **No page-wide reveal** — see above. The clip-path animation plays inside
//!   the button.
//! - **`circle-blur` fades instead of blurring.** frust's scene has no blur
//!   primitive, so the variant keeps its circular expand and swaps the
//!   `blur(8px) → blur(0px)` half for an opacity ramp on the incoming icon.
//! - **The circle variants' easing is substituted.** Upstream uses the Material
//!   standard curve `cubic-bezier(0.4, 0, 0.2, 1)`, which is not a beUI token;
//!   the catalog's own symmetric [`EASE_IN_OUT`] drives them instead, and the
//!   rectangle and blinds variants keep upstream's [`EASE_OUT`].
//! - **Four slats, not a 72px tile.** The blinds mask repeats every 72px across
//!   a viewport; scoped to a 32px button that is one slat, so the port fixes the
//!   count at [`BLIND_SLATS`] instead.
//! - **Icons are drawn, not Lucide glyphs** — the same route every other icon in
//!   this catalog takes.

use std::f64::consts::TAU;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape, Size, Toggled, TypedArgCallback,
    View, Widget, erase_callback_arg,
};
use frust::{Brightness, FrameTime, Theme};

use crate::motion::Ramp;
use crate::press::{inside, is_activation_key, presses};
use crate::style::{
    ACTIVE_CURSOR, PATH_TOLERANCE, PRESS_SCALE, RADIUS_ICON_BUTTON, SIZE_ICON_BUTTON,
    resolve_radius, scale_alpha,
};
use crate::tokens::BEUI_LIGHT;
use crate::tokens::motion::{EASE_IN_OUT, EASE_OUT, SPRING_PRESS};

/// How long a rectangle wipe takes — `beui-rect-reveal 400ms`.
pub const RECT_REVEAL: Duration = Duration::from_millis(400);

/// How long a circular or slatted reveal takes — `700ms`.
pub const CIRCLE_REVEAL: Duration = Duration::from_millis(700);

/// How many slats the blinds variant opens across the button. Upstream tiles a
/// 72px mask across the viewport; scoped to a button, the count is fixed here.
pub const BLIND_SLATS: usize = 4;

/// How far past the box a circular reveal grows — `circle(150%)`.
const CIRCLE_OVERSHOOT: f64 = 1.5;

/// The sun's disc radius, as a fraction of the icon box.
const SUN_DISC: f64 = 0.24;
/// Where a sun ray starts and ends, as fractions of the icon box.
const SUN_RAY: (f64, f64) = (0.34, 0.46);
/// How many rays the sun carries — Lucide's `Sun` has eight.
const SUN_RAYS: usize = 8;

/// The moon's outer radius, as a fraction of the icon box.
const MOON_RADIUS: f64 = 0.42;
/// The bite-out circle's radius, relative to the moon's own.
const MOON_BITE_RADIUS: f64 = 0.9;
/// How far the bite-out circle sits from the moon's centre, relative to the
/// moon's radius.
const MOON_BITE_OFFSET: f64 = 0.55;
/// How far the crescent is turned, so it opens toward the upper right rather
/// than flat to the right.
const MOON_TILT: f64 = -0.65;
/// How many points each of the crescent's two arcs is sampled at.
const MOON_SAMPLES: usize = 24;

/// Unthemed fallback ink (beUI light `--foreground`).
const FALLBACK_INK: Color = BEUI_LIGHT.foreground;

/// How the new icon is revealed over the old — upstream's `ThemeVariant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeToggleVariant {
    /// `rectangle`: a rectangular wipe from [`ThemeToggleStart`]'s corner or
    /// edge.
    #[default]
    Rectangle,
    /// `circle`: a circular expand from [`ThemeToggleStart`]'s origin.
    Circle,
    /// `circle-blur`: the same expand, with the incoming icon fading in — the
    /// port's stand-in for upstream's `blur(8px) → blur(0px)`.
    CircleBlur,
    /// `blinds`: [`BLIND_SLATS`] bands opening together like a shutter.
    Blinds,
}

impl ThemeToggleVariant {
    /// Every variant, in upstream's own declaration order.
    pub const ALL: [ThemeToggleVariant; 4] = [
        ThemeToggleVariant::Rectangle,
        ThemeToggleVariant::Circle,
        ThemeToggleVariant::CircleBlur,
        ThemeToggleVariant::Blinds,
    ];

    /// How long this variant's reveal runs — upstream's per-variant durations.
    pub fn duration(self) -> Duration {
        match self {
            ThemeToggleVariant::Rectangle => RECT_REVEAL,
            _ => CIRCLE_REVEAL,
        }
    }

    /// The ramp this variant's reveal runs on. See the [module docs](self) on
    /// the circle variants' substituted curve.
    fn ramp(self) -> Ramp {
        match self {
            ThemeToggleVariant::Circle | ThemeToggleVariant::CircleBlur => {
                Ramp::eased(self.duration(), EASE_IN_OUT)
            }
            _ => Ramp::eased(self.duration(), EASE_OUT),
        }
    }
}

/// Where a reveal starts from — upstream's `RectStart`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeToggleStart {
    /// From the top-left corner.
    TopLeft,
    /// From the top-right corner.
    TopRight,
    /// From the bottom-left corner.
    BottomLeft,
    /// From the bottom-right corner.
    BottomRight,
    /// Outward from the middle.
    Center,
    /// Upward from the bottom edge — upstream's default.
    #[default]
    BottomUp,
}

impl ThemeToggleStart {
    /// Every origin, in upstream's own declaration order.
    pub const ALL: [ThemeToggleStart; 6] = [
        ThemeToggleStart::TopLeft,
        ThemeToggleStart::TopRight,
        ThemeToggleStart::BottomLeft,
        ThemeToggleStart::BottomRight,
        ThemeToggleStart::Center,
        ThemeToggleStart::BottomUp,
    ];

    /// The rectangle wipe's starting inset — `RECT_FROM`, as
    /// `(top, right, bottom, left)` fractions of the box.
    pub fn rect_inset(self) -> (f64, f64, f64, f64) {
        match self {
            ThemeToggleStart::TopLeft => (0.0, 1.0, 1.0, 0.0),
            ThemeToggleStart::TopRight => (0.0, 0.0, 1.0, 1.0),
            ThemeToggleStart::BottomLeft => (1.0, 1.0, 0.0, 0.0),
            ThemeToggleStart::BottomRight => (1.0, 0.0, 0.0, 1.0),
            ThemeToggleStart::Center => (0.5, 0.5, 0.5, 0.5),
            ThemeToggleStart::BottomUp => (1.0, 0.0, 0.0, 0.0),
        }
    }

    /// The circular reveal's origin — `CIRCLE_ORIGIN`, as fractions of the box.
    pub fn circle_origin(self) -> (f64, f64) {
        match self {
            ThemeToggleStart::TopLeft => (0.0, 0.0),
            ThemeToggleStart::TopRight => (1.0, 0.0),
            ThemeToggleStart::BottomLeft => (0.0, 1.0),
            ThemeToggleStart::BottomRight => (1.0, 1.0),
            ThemeToggleStart::Center => (0.5, 0.5),
            ThemeToggleStart::BottomUp => (0.5, 1.0),
        }
    }
}

/// A declarative beUI theme toggle. See the [module docs](self).
///
/// # Example
///
/// ```
/// use frust_beui::components::theme_toggle::{ThemeToggleVariant, theme_toggle};
///
/// let toggle = theme_toggle::<()>().variant(ThemeToggleVariant::Circle);
/// ```
pub struct ThemeToggleView<State: 'static> {
    variant: ThemeToggleVariant,
    start: ThemeToggleStart,
    size: f64,
    brightness: Option<Brightness>,
    drive_app_theme: bool,
    on_toggle: TypedArgCallback<State, Brightness>,
}

/// Create a theme toggle: upstream's defaults, a
/// [`Rectangle`](ThemeToggleVariant::Rectangle) reveal running
/// [`BottomUp`](ThemeToggleStart::BottomUp), driving the app theme itself.
pub fn theme_toggle<State: 'static>() -> ThemeToggleView<State> {
    ThemeToggleView {
        variant: ThemeToggleVariant::default(),
        start: ThemeToggleStart::default(),
        size: SIZE_ICON_BUTTON,
        brightness: None,
        drive_app_theme: true,
        on_toggle: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> ThemeToggleView<State> {
    /// Reveal with `variant` instead of [`ThemeToggleVariant::Rectangle`].
    pub fn variant(mut self, variant: ThemeToggleVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Start the reveal from `start` instead of [`ThemeToggleStart::BottomUp`].
    pub fn start(mut self, start: ThemeToggleStart) -> Self {
        self.start = start;
        self
    }

    /// Set the square edge, in logical px (default [`SIZE_ICON_BUTTON`], the
    /// catalog's icon-button metric).
    pub fn size(mut self, size: f64) -> Self {
        self.size = size.max(0.0);
        self
    }

    /// Show `brightness` regardless of what the ambient theme reports — the
    /// controlled form, for an app that already owns the value.
    ///
    /// Unset, the toggle reads the theme it is painted under, which is what
    /// makes the default (self-driving) form correct without any app state.
    pub fn brightness(mut self, brightness: Brightness) -> Self {
        self.brightness = Some(brightness);
        self
    }

    /// Whether activating the toggle pushes the new theme itself through
    /// [`frust::set_app_theme`] (default `true`).
    ///
    /// Turn it off in an app that owns its own theming: the toggle then only
    /// reports through [`on_toggle`](Self::on_toggle), and the app decides what
    /// a brightness change means (see the [module docs](self)).
    pub fn drive_app_theme(mut self, drive: bool) -> Self {
        self.drive_app_theme = drive;
        self
    }

    /// Called with the brightness the user asked for, before any theme push.
    pub fn on_toggle(mut self, callback: impl Fn(&mut State, Brightness) + 'static) -> Self {
        self.on_toggle = Rc::new(callback);
        self
    }
}

/// One spring-driven scalar with retarget-from-here semantics — the press
/// scale's whole state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sprung {
    from: f64,
    to: f64,
    started: Option<FrameTime>,
}

impl Sprung {
    /// A value resting at `at`.
    fn resting(at: f64) -> Self {
        Sprung {
            from: at,
            to: at,
            started: None,
        }
    }

    /// Where it sits at `now`.
    fn value(&self, now: FrameTime) -> f64 {
        let Some(started) = self.started else {
            return self.from;
        };
        let progress = Ramp::spring(SPRING_PRESS).progress(now.saturating_sub(started));
        self.from + (self.to - self.from) * progress
    }

    /// Re-aim at `to` **from wherever it currently is**, so a press released
    /// mid-travel springs back continuously instead of snapping.
    fn retarget(&mut self, now: FrameTime, to: f64) {
        if self.to == to {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.started = Some(now);
    }

    /// Whether it has arrived by `now`.
    fn is_settled(&self, now: FrameTime) -> bool {
        match self.started {
            None => true,
            Some(started) => {
                self.from == self.to
                    || Ramp::spring(SPRING_PRESS).is_settled(now.saturating_sub(started))
            }
        }
    }
}

/// The retained widget for a [`ThemeToggleView`].
pub struct ThemeToggleWidget {
    variant: ThemeToggleVariant,
    start: ThemeToggleStart,
    size: f64,
    /// The caller's pinned brightness, when the toggle is controlled.
    pinned: Option<Brightness>,
    /// The brightness the last paint resolved — what `event` toggles away from.
    resolved: Brightness,
    /// The brightness being revealed away from, held for the reveal's length.
    outgoing: Option<Brightness>,
    /// When the reveal started, latched on its first paint.
    reveal: Option<FrameTime>,
    drive_app_theme: bool,
    press: Sprung,
    /// Armed by a `Down` inside, cleared on `Up`/`Cancel`.
    captured: bool,
    /// A press state change waiting for a clock — only `paint` has one.
    press_pending: bool,
    on_toggle: ErasedArgCallback<Brightness>,
}

impl ThemeToggleWidget {
    /// The brightness currently shown.
    pub fn brightness(&self) -> Brightness {
        self.pinned.unwrap_or(self.resolved)
    }

    /// The brightness activating the toggle asks for.
    pub fn next_brightness(&self) -> Brightness {
        match self.brightness() {
            Brightness::Light => Brightness::Dark,
            Brightness::Dark => Brightness::Light,
        }
    }

    /// The accessible name — upstream's own two strings.
    pub fn action_label(&self) -> &'static str {
        match self.brightness() {
            Brightness::Dark => "Switch to light mode",
            Brightness::Light => "Switch to dark mode",
        }
    }

    /// Whether a reveal is in flight, and how far along it is.
    fn reveal_progress(&mut self, now: FrameTime, reduce: bool) -> Option<f64> {
        if self.outgoing.is_none() || reduce {
            self.outgoing = None;
            self.reveal = None;
            return None;
        }
        let started = *self.reveal.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        let ramp = self.variant.ramp();
        if ramp.is_settled(elapsed) {
            self.outgoing = None;
            self.reveal = None;
            return None;
        }
        Some(ramp.progress_clamped(elapsed))
    }

    /// Report the user's choice and, unless the app took the job, push the new
    /// theme.
    fn activate(&mut self, ctx: &mut EventCtx) {
        let next = self.next_brightness();
        (self.on_toggle)(ctx, next);
        if self.drive_app_theme {
            // The catalogs' own demo mechanism: a builder over this design
            // system's theme with the wanted brightness pinned, pushed through
            // the app-override slot.
            frust::set_app_theme(Theme::builder(crate::theme()).brightness(next).build());
        }
        // The reveal plays from here whether or not the ambient theme follows;
        // a controlled toggle whose owner refuses the change simply reveals
        // back to where it was.
        self.outgoing = Some(self.brightness());
        self.reveal = None;
        self.resolved = next;
        ctx.request_redraw();
    }
}

impl<State: 'static> View<State> for ThemeToggleView<State> {
    type Element = ThemeToggleWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ThemeToggleWidget {
        ThemeToggleWidget {
            variant: self.variant,
            start: self.start,
            size: self.size,
            pinned: self.brightness,
            resolved: self.brightness.unwrap_or(Brightness::Light),
            outgoing: None,
            reveal: None,
            drive_app_theme: self.drive_app_theme,
            press: Sprung::resting(1.0),
            captured: false,
            press_pending: false,
            on_toggle: erase_callback_arg(&self.on_toggle),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ThemeToggleWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_toggle = erase_callback_arg(&self.on_toggle);
        let mut flags = ChangeFlags::NONE;
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::PAINT;
        }
        if prev.start != self.start {
            element.start = self.start;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.drive_app_theme != self.drive_app_theme {
            element.drive_app_theme = self.drive_app_theme;
        }
        if element.pinned != self.brightness {
            // The app is the source of truth for a controlled toggle: adopt the
            // confirmed value, revealing from whatever was on screen.
            if let Some(brightness) = self.brightness
                && brightness != element.brightness()
            {
                element.outgoing = Some(element.brightness());
                element.reveal = None;
            }
            element.pinned = self.brightness;
            flags |= ChangeFlags::PAINT;
        }
        flags
    }
}

impl Widget for ThemeToggleWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(self.size, self.size))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let ink = theme.map_or(FALLBACK_INK, |t| t.scheme().on_surface);
        // An uncontrolled toggle shows whatever the ambient theme reports, which
        // is what makes the self-driving form correct with no app state.
        if self.pinned.is_none()
            && self.outgoing.is_none()
            && let Some(theme) = theme
        {
            self.resolved = theme.brightness;
        }
        let now = ctx.frame_time();
        if self.press_pending {
            self.press_pending = false;
            self.press
                .retarget(now, if self.captured { PRESS_SCALE } else { 1.0 });
        }
        let scale = if reduce {
            if self.captured { PRESS_SCALE } else { 1.0 }
        } else {
            self.press.value(now)
        };

        let origin = ctx.origin();
        let size = ctx.size();
        let radius = resolve_radius(RADIUS_ICON_BUTTON, size.width, size.height);
        if ctx.has_focus() {
            // The one piece of chrome the button paints: a focus ring, on the
            // catalog's own ring colour.
            let ring = crate::tokens::BeuiTokens::resolve_ring(None, theme);
            let outline = RoundedRect::from_rect(
                Rect::from_origin_size(Point::ORIGIN, size).inset(1.0),
                radius + 1.0,
            );
            scene.stroke_path(
                origin,
                &Shape::to_path(&outline, PATH_TOLERANCE),
                crate::style::FOCUS_RING_WIDTH,
                &Brush::Solid(scale_alpha(ring, crate::style::FOCUS_RING_OPACITY)),
            );
        }

        let progress = self.reveal_progress(now, reduce);
        let centre = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        scene.push_transform(
            Affine::translate(centre.to_vec2())
                * Affine::scale(scale)
                * Affine::translate(-centre.to_vec2()),
        );

        match progress {
            None => draw_icon(scene, origin, size, self.brightness(), ink),
            Some(progress) => {
                // The old icon is whole underneath; the new one is revealed over
                // it through the variant's own clip.
                if let Some(previous) = self.outgoing {
                    draw_icon(scene, origin, size, previous, ink);
                }
                let alpha = if self.variant == ThemeToggleVariant::CircleBlur {
                    // The stand-in for upstream's blur half.
                    progress as f32
                } else {
                    1.0
                };
                let incoming = scale_alpha(ink, alpha);
                for region in reveal_regions(self.variant, self.start, size, progress) {
                    scene.push_clip_rounded(
                        Point::new(origin.x + region.origin.x, origin.y + region.origin.y),
                        region.size,
                        region.radius,
                    );
                    draw_icon(scene, origin, size, self.brightness(), incoming);
                    scene.pop_clip();
                }
            }
        }
        scene.pop_transform();

        if reduce {
            return;
        }
        if progress.is_some() || !self.press.is_settled(now) {
            // Both are runs with a visible endpoint.
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) if is_activation_key(key) => {
                self.activate(ctx);
                EventResult::Handled
            }
            InputEvent::Pointer(pointer) => {
                let size = ctx.size();
                match pointer.phase {
                    PointerPhase::Down => {
                        if !presses(pointer) || !inside(pointer.position, size) {
                            return EventResult::Ignored;
                        }
                        self.captured = true;
                        self.press_pending = true;
                        ctx.capture_pointer();
                        ctx.request_focus();
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                    PointerPhase::Move => {
                        if !self.captured {
                            if inside(pointer.position, size) {
                                ctx.claim_hover();
                                ctx.set_cursor(ACTIVE_CURSOR);
                            }
                            // Watching a move is not consuming it.
                            return EventResult::Ignored;
                        }
                        ctx.set_cursor(ACTIVE_CURSOR);
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        self.press_pending = true;
                        if inside(pointer.position, size) {
                            self.activate(ctx);
                        } else {
                            ctx.request_redraw();
                        }
                        EventResult::Handled
                    }
                    PointerPhase::Cancel => {
                        if !self.captured {
                            return EventResult::Ignored;
                        }
                        self.captured = false;
                        self.press_pending = true;
                        ctx.request_redraw();
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // ARIA's toggle button: a Button node carrying the pressed state, with
        // upstream's own two labels.
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.action_label());
            node.set_toggled(Toggled::from(self.brightness() == Brightness::Dark));
            node.add_action(Action::Click);
        });
    }
}

/// One revealed region: a rounded rectangle in the widget's own space. A circle
/// is a square whose radius is half its edge.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RevealRegion {
    origin: Point,
    size: Size,
    radius: f64,
}

/// The regions the new icon is drawn through at `progress` — one for the wipe
/// and circle variants, [`BLIND_SLATS`] for the shutter.
fn reveal_regions(
    variant: ThemeToggleVariant,
    start: ThemeToggleStart,
    size: Size,
    progress: f64,
) -> Vec<RevealRegion> {
    let progress = progress.clamp(0.0, 1.0);
    match variant {
        ThemeToggleVariant::Rectangle => {
            // The starting inset relaxes to zero, which is exactly the
            // `clip-path: inset(...)` keyframe.
            let (top, right, bottom, left) = start.rect_inset();
            let open = 1.0 - progress;
            let x = left * open * size.width;
            let y = top * open * size.height;
            let width = (size.width * (1.0 - (left + right) * open)).max(0.0);
            let height = (size.height * (1.0 - (top + bottom) * open)).max(0.0);
            vec![RevealRegion {
                origin: Point::new(x, y),
                size: Size::new(width, height),
                radius: 0.0,
            }]
        }
        ThemeToggleVariant::Circle | ThemeToggleVariant::CircleBlur => {
            let (fx, fy) = start.circle_origin();
            let centre = Point::new(fx * size.width, fy * size.height);
            let full = size.width.max(size.height) * CIRCLE_OVERSHOOT;
            let radius = full * progress;
            vec![RevealRegion {
                origin: Point::new(centre.x - radius, centre.y - radius),
                size: Size::new(radius * 2.0, radius * 2.0),
                radius,
            }]
        }
        ThemeToggleVariant::Blinds => {
            let slat = size.width / BLIND_SLATS as f64;
            (0..BLIND_SLATS)
                .map(|index| RevealRegion {
                    origin: Point::new(index as f64 * slat, 0.0),
                    size: Size::new(slat * progress, size.height),
                    radius: 0.0,
                })
                .collect()
        }
    }
}

/// Draw the icon for `brightness`: a sun when the theme is dark (tapping goes to
/// light), a moon when it is light — upstream's own pairing.
fn draw_icon(
    scene: &mut dyn PaintScene,
    origin: Point,
    size: Size,
    brightness: Brightness,
    ink: Color,
) {
    let edge = size.width.min(size.height);
    if edge <= 0.0 {
        return;
    }
    let centre = Point::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
    // Lucide strokes at 2px on a 24px canvas.
    let stroke = (edge / 24.0 * 2.0).max(1.0);
    match brightness {
        Brightness::Dark => {
            let disc = edge * SUN_DISC;
            scene.fill_rounded_rect(
                Point::new(centre.x - disc / 2.0, centre.y - disc / 2.0),
                Size::new(disc, disc),
                disc / 2.0,
                ink,
            );
            let mut rays = BezPath::new();
            for index in 0..SUN_RAYS {
                let angle = index as f64 / SUN_RAYS as f64 * TAU;
                let (sin, cos) = angle.sin_cos();
                rays.move_to(Point::new(
                    centre.x + cos * edge * SUN_RAY.0,
                    centre.y + sin * edge * SUN_RAY.0,
                ));
                rays.line_to(Point::new(
                    centre.x + cos * edge * SUN_RAY.1,
                    centre.y + sin * edge * SUN_RAY.1,
                ));
            }
            scene.stroke_path(Point::ORIGIN, &rays, stroke, &Brush::Solid(ink));
        }
        Brightness::Light => {
            scene.fill_path(
                Point::ORIGIN,
                &moon_path(centre, edge * MOON_RADIUS),
                &Brush::Solid(ink),
            );
        }
    }
}

/// A crescent: the part of a disc of `radius` about `centre` that lies outside a
/// second, offset disc.
///
/// Both boundary arcs are sampled rather than emitted as arc segments, because
/// the closing arc has to be traversed *backwards* and a sampled polyline is the
/// cheapest correct way to do that at this size (the whole glyph is ~14px).
fn moon_path(centre: Point, radius: f64) -> BezPath {
    let bite_radius = radius * MOON_BITE_RADIUS;
    let bite_offset = radius * MOON_BITE_OFFSET;
    // The two circles meet where `x` satisfies the radical-line equation; `y`
    // follows from the outer circle.
    let x = (bite_offset * bite_offset + radius * radius - bite_radius * bite_radius)
        / (2.0 * bite_offset);
    let y_squared = radius * radius - x * x;
    let mut path = BezPath::new();
    if y_squared <= 0.0 || !y_squared.is_finite() {
        // Degenerate geometry: fall back to the whole disc rather than an empty
        // path, so the icon is never invisible.
        for index in 0..=MOON_SAMPLES * 2 {
            let angle = index as f64 / (MOON_SAMPLES * 2) as f64 * TAU;
            let at = tilted(centre, radius, angle);
            if index == 0 {
                path.move_to(at);
            } else {
                path.line_to(at);
            }
        }
        path.close_path();
        return path;
    }
    let y = y_squared.sqrt();
    let outer_angle = y.atan2(x);
    let inner_angle = y.atan2(x - bite_offset);

    // The outer boundary: the long way round, away from the bite.
    for index in 0..=MOON_SAMPLES {
        let t = index as f64 / MOON_SAMPLES as f64;
        let angle = outer_angle + t * (TAU - 2.0 * outer_angle);
        let at = tilted(centre, radius, angle);
        if index == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    // The inner boundary, traversed back to the start.
    let bite_centre = Point::new(centre.x + bite_offset, centre.y);
    for index in 0..=MOON_SAMPLES {
        let t = index as f64 / MOON_SAMPLES as f64;
        let angle = (TAU - inner_angle) + t * (2.0 * inner_angle - TAU);
        let raw = Point::new(
            bite_centre.x + angle.cos() * bite_radius,
            bite_centre.y + angle.sin() * bite_radius,
        );
        path.line_to(rotate_about(raw, centre, MOON_TILT));
    }
    path.close_path();
    path
}

/// A point on a circle of `radius` about `centre` at `angle`, turned by
/// [`MOON_TILT`] so the crescent opens toward the upper right.
fn tilted(centre: Point, radius: f64, angle: f64) -> Point {
    rotate_about(
        Point::new(
            centre.x + angle.cos() * radius,
            centre.y + angle.sin() * radius,
        ),
        centre,
        MOON_TILT,
    )
}

/// `point` turned `angle` radians about `pivot`.
fn rotate_about(point: Point, pivot: Point, angle: f64) -> Point {
    let (sin, cos) = angle.sin_cos();
    let dx = point.x - pivot.x;
    let dy = point.y - pivot.y;
    Point::new(pivot.x + dx * cos - dy * sin, pivot.y + dx * sin + dy * cos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::{Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent};
    use frust_core::{BuildCtx, EventCtx, PaintCtx};
    use std::any::Any;

    /// The box every paint test lays the toggle into.
    const BOX: Size = Size::new(SIZE_ICON_BUTTON, SIZE_ICON_BUTTON);

    /// The app state the toggle reports into.
    #[derive(Default)]
    struct Toggles {
        asked: Vec<Brightness>,
    }

    /// Records the icon draws and the reveal clips.
    #[derive(Default)]
    struct Recorder {
        discs: usize,
        paths: usize,
        strokes: usize,
        clips: Vec<(Point, Size, f64)>,
    }

    impl Recorder {
        /// How many icons this paint drew: a sun is a disc plus its rays, a
        /// moon is one filled path.
        fn icons(&self) -> usize {
            self.discs + self.paths
        }
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, _o: Point, _s: Size, _r: f64, _color: Color) {
            self.discs += 1;
        }
        fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {
            self.paths += 1;
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, _brush: &Brush) {
            self.strokes += 1;
        }
        fn push_clip_rounded(&mut self, origin: Point, size: Size, radius: f64) {
            self.clips.push((origin, size, radius));
        }
    }

    fn built(view: &ThemeToggleView<Toggles>) -> ThemeToggleWidget {
        let mut next_id = 0u64;
        let mut widget = View::<Toggles>::build(view, &mut BuildCtx::new(&mut next_id));
        let mut layout = LayoutCtx::new();
        widget.layout(&mut layout, &BoxConstraints::loose(BOX));
        widget
    }

    fn painted(widget: &mut ThemeToggleWidget, ms: u64, theme: Option<&Theme>) -> (Recorder, bool) {
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, BOX, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut recorder = Recorder::default();
        widget.paint(&mut ctx, &mut recorder);
        (recorder, ctx.needs_frame())
    }

    fn dispatch(
        widget: &mut ThemeToggleWidget,
        state: &mut Toggles,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, BOX);
        widget.event(&mut ctx, event)
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn enter_key() -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        })
    }

    fn recording() -> ThemeToggleView<Toggles> {
        theme_toggle::<Toggles>()
            .drive_app_theme(false)
            .on_toggle(|state: &mut Toggles, brightness| state.asked.push(brightness))
    }

    fn reduced() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    /// Every variant and every origin upstream's props accept is constructible
    /// and paints an icon.
    #[test]
    fn every_variant_and_origin_is_constructible() {
        assert_eq!(ThemeToggleVariant::ALL.len(), 4);
        assert_eq!(ThemeToggleStart::ALL.len(), 6);
        for variant in ThemeToggleVariant::ALL {
            for start in ThemeToggleStart::ALL {
                let mut widget = built(&recording().variant(variant).start(start));
                let (recorder, _) = painted(&mut widget, 0, None);
                assert_eq!(recorder.icons(), 1, "{variant:?}/{start:?}");
            }
        }
    }

    /// The clip-path tables are upstream's, transcribed.
    #[test]
    fn the_reveal_origin_tables_are_the_source_tables() {
        assert_eq!(
            ThemeToggleStart::BottomUp.rect_inset(),
            (1.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(ThemeToggleStart::TopLeft.rect_inset(), (0.0, 1.0, 1.0, 0.0));
        assert_eq!(ThemeToggleStart::Center.rect_inset(), (0.5, 0.5, 0.5, 0.5));
        assert_eq!(ThemeToggleStart::BottomUp.circle_origin(), (0.5, 1.0));
        assert_eq!(ThemeToggleStart::TopRight.circle_origin(), (1.0, 0.0));
        assert_eq!(ThemeToggleStart::default(), ThemeToggleStart::BottomUp);
        assert_eq!(ThemeToggleVariant::default(), ThemeToggleVariant::Rectangle);
        assert_eq!(ThemeToggleVariant::Rectangle.duration(), RECT_REVEAL);
        assert_eq!(ThemeToggleVariant::Blinds.duration(), CIRCLE_REVEAL);
    }

    /// A reveal opens from nothing to the whole box, monotonically, for every
    /// variant and origin — the property that keeps the swap from flashing.
    #[test]
    fn every_reveal_opens_from_nothing_to_everything() {
        for variant in ThemeToggleVariant::ALL {
            for start in ThemeToggleStart::ALL {
                let area = |progress: f64| {
                    reveal_regions(variant, start, BOX, progress)
                        .iter()
                        .map(|region| region.size.width * region.size.height)
                        .sum::<f64>()
                };
                assert_eq!(area(0.0), 0.0, "{variant:?}/{start:?} starts closed");
                let mut previous = 0.0;
                for step in 0..=20 {
                    let at = area(f64::from(step) / 20.0);
                    assert!(
                        at >= previous - 1e-9,
                        "{variant:?}/{start:?} went backwards"
                    );
                    previous = at;
                }
                assert!(
                    area(1.0) >= BOX.width * BOX.height - 1e-9,
                    "{variant:?}/{start:?} never covers the box"
                );
            }
        }
    }

    /// The shutter opens one region per slat; every other variant reveals
    /// through a single one.
    #[test]
    fn the_shutter_opens_one_region_per_slat() {
        let slats = reveal_regions(
            ThemeToggleVariant::Blinds,
            ThemeToggleStart::default(),
            BOX,
            0.5,
        );
        assert_eq!(slats.len(), BLIND_SLATS);
        for (index, slat) in slats.iter().enumerate() {
            assert_eq!(slat.origin.x, index as f64 * BOX.width / BLIND_SLATS as f64);
            assert_eq!(slat.size.height, BOX.height, "a slat spans the box");
        }
        for variant in [
            ThemeToggleVariant::Rectangle,
            ThemeToggleVariant::Circle,
            ThemeToggleVariant::CircleBlur,
        ] {
            assert_eq!(
                reveal_regions(variant, ThemeToggleStart::default(), BOX, 0.5).len(),
                1,
                "{variant:?}"
            );
        }
        // A circular reveal is a square clipped to half its own edge.
        let circle = reveal_regions(
            ThemeToggleVariant::Circle,
            ThemeToggleStart::Center,
            BOX,
            0.5,
        )[0];
        assert_eq!(circle.radius * 2.0, circle.size.width);
        assert_eq!(circle.size.width, circle.size.height);
    }

    /// Activating asks for the other brightness, and announces itself with
    /// upstream's own two labels.
    #[test]
    fn activating_asks_for_the_other_brightness() {
        let mut state = Toggles::default();
        let mut widget = built(&recording().brightness(Brightness::Light));
        assert_eq!(widget.brightness(), Brightness::Light);
        assert_eq!(widget.next_brightness(), Brightness::Dark);
        assert_eq!(widget.action_label(), "Switch to dark mode");

        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, 8.0, 8.0),
        );
        assert_eq!(state.asked, vec![Brightness::Dark]);

        // Keyboard activation reports the same thing.
        dispatch(&mut widget, &mut state, &enter_key());
        assert_eq!(state.asked.len(), 2);
    }

    /// A press released outside the control is abandoned, and a press that
    /// never started is not a press at all.
    #[test]
    fn a_press_released_outside_does_not_toggle() {
        let mut state = Toggles::default();
        let mut widget = built(&recording());

        // No `Down` first: the release belongs to somebody else.
        assert_eq!(
            dispatch(
                &mut widget,
                &mut state,
                &pointer(PointerPhase::Up, 8.0, 8.0)
            ),
            EventResult::Ignored
        );
        assert!(state.asked.is_empty());

        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, 900.0, 900.0),
        );
        assert!(state.asked.is_empty(), "released off the control");

        // ...and a cancel drops it too.
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Cancel, 8.0, 8.0),
        );
        assert!(state.asked.is_empty());
    }

    /// The press spring shrinks the control and springs back from wherever it
    /// got to — a release mid-travel never snaps.
    #[test]
    fn the_press_spring_shrinks_and_returns_without_jumping() {
        let now = FrameTime::from_nanos(0);
        let mut press = Sprung::resting(1.0);
        assert!(press.is_settled(now));

        press.retarget(now, PRESS_SCALE);
        assert!(!press.is_settled(now));
        let mid_time = FrameTime::from_nanos(20_000_000);
        let mid = press.value(mid_time);
        assert!(mid < 1.0 && mid > PRESS_SCALE, "mid-press scale: {mid}");

        // Releasing re-aims from exactly here.
        press.retarget(mid_time, 1.0);
        assert!(
            (press.value(mid_time) - mid).abs() < 1e-9,
            "the press jumped"
        );
        assert!(press.value(FrameTime::from_nanos(2_000_000_000)) >= 1.0 - 1e-9);
    }

    /// A press asks for frames while the spring runs, and stops once it lands.
    #[test]
    fn a_press_drives_frames_until_the_spring_settles() {
        let mut state = Toggles::default();
        let mut widget = built(&recording());
        painted(&mut widget, 0, None);
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        let (_, needs_frame) = painted(&mut widget, 1, None);
        assert!(needs_frame, "the press spring is running");
        let (_, needs_frame) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame);
    }

    /// A toggle reveals the new icon over the old, and settles back to one.
    #[test]
    fn toggling_reveals_the_new_icon_over_the_old() {
        let mut state = Toggles::default();
        let mut widget = built(&recording().brightness(Brightness::Light));
        let (before, _) = painted(&mut widget, 0, None);
        assert_eq!(before.icons(), 1);
        assert!(before.clips.is_empty());

        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, 8.0, 8.0),
        );
        let (mid, needs_frame) = painted(&mut widget, 100, None);
        assert!(needs_frame, "the reveal is running");
        assert_eq!(mid.icons(), 2, "both icons on screen");
        assert_eq!(mid.clips.len(), 1, "the incoming one is clipped");

        let (settled, needs_frame) = painted(&mut widget, 5_000, None);
        assert!(!needs_frame);
        assert_eq!(settled.icons(), 1);
        assert!(settled.clips.is_empty());
    }

    /// An uncontrolled toggle follows the theme it is painted under, which is
    /// what makes the self-driving form correct with no app state.
    #[test]
    fn an_uncontrolled_toggle_reads_the_ambient_theme() {
        let mut widget = built(&recording());
        let mut dark = crate::theme();
        dark.brightness = Brightness::Dark;
        painted(&mut widget, 0, Some(&dark));
        assert_eq!(widget.brightness(), Brightness::Dark);
        assert_eq!(widget.action_label(), "Switch to light mode");
        assert_eq!(widget.next_brightness(), Brightness::Light);

        let mut light = crate::theme();
        light.brightness = Brightness::Light;
        painted(&mut widget, 1, Some(&light));
        assert_eq!(widget.brightness(), Brightness::Light);
    }

    /// A controlled toggle adopts the app's confirmed value and reveals into it.
    #[test]
    fn a_controlled_toggle_reveals_into_the_confirmed_value() {
        let first = recording().brightness(Brightness::Light);
        let mut widget = built(&first);
        painted(&mut widget, 0, None);

        let second = recording().brightness(Brightness::Dark);
        let mut next_id = 0u64;
        View::<Toggles>::rebuild(
            &second,
            &first,
            &mut widget,
            &mut BuildCtx::new(&mut next_id),
        );
        assert_eq!(widget.brightness(), Brightness::Dark);
        let (mid, needs_frame) = painted(&mut widget, 10, None);
        assert!(needs_frame);
        assert_eq!(mid.icons(), 2, "the old icon is still on screen");
    }

    /// `reduce_motion` swaps the icon outright — no reveal, no press spring, no
    /// frames.
    #[test]
    fn reduce_motion_swaps_the_icon_outright() {
        let theme = reduced();
        let mut state = Toggles::default();
        let mut widget = built(&recording().brightness(Brightness::Light));
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, 8.0, 8.0),
        );
        let (recorder, needs_frame) = painted(&mut widget, 0, Some(&theme));
        assert!(!needs_frame);
        assert_eq!(recorder.icons(), 1);
        assert!(recorder.clips.is_empty());
    }

    /// The self-driving form really does push a theme — the mechanism the
    /// module docs name.
    #[test]
    fn the_self_driving_form_pushes_an_app_theme() {
        let mut state = Toggles::default();
        let mut widget = built(
            &theme_toggle::<Toggles>()
                .brightness(Brightness::Light)
                .on_toggle(|state: &mut Toggles, brightness| state.asked.push(brightness)),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Down, 8.0, 8.0),
        );
        dispatch(
            &mut widget,
            &mut state,
            &pointer(PointerPhase::Up, 8.0, 8.0),
        );
        assert_eq!(state.asked, vec![Brightness::Dark]);
        // The push is a process-global override; hand the slot back so it does
        // not outlive this test.
        frust::clear_app_theme();
    }

    /// The crescent is a real closed path with both of its arcs, not a disc —
    /// the moon has to read as a moon at 14px.
    #[test]
    fn the_moon_is_a_crescent_not_a_disc() {
        let centre = Point::new(16.0, 16.0);
        let radius = 8.0;
        let path = moon_path(centre, radius);
        let elements = path.elements().len();
        assert!(
            elements >= MOON_SAMPLES * 2,
            "both arcs are sampled: {elements}"
        );
        // Every point is inside the outer disc...
        for segment in path.segments() {
            let at = segment.as_line().map(|line| line.p1);
            if let Some(at) = at {
                let distance = ((at.x - centre.x).powi(2) + (at.y - centre.y).powi(2)).sqrt();
                assert!(distance <= radius + 1e-6, "escaped the disc: {distance}");
            }
        }
        // ...and the bounding box is narrower than a full disc's, because the
        // bite has been taken out of one side.
        let bounds = path.bounding_box();
        assert!(
            bounds.width() < radius * 2.0 || bounds.height() < radius * 2.0,
            "no bite was taken: {bounds:?}"
        );
    }
}
