//! Ports beUI's `button` component — the registry slug's four sources in one
//! widget: `components/motion/button/{base,metallic,magnetic,stateful}.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01).
//!
//! # Two axes, not one
//!
//! Upstream ships four *components* (`Button`, `MetallicButton`,
//! `MagneticButton`, `StatefulButton`) that all wrap the same base, and the base
//! itself carries a `variant` prop selecting its fill treatment. A port with one
//! `variant` enum would have to collapse the two, so this widget keeps them
//! apart:
//!
//! - [`ButtonVariant`] is the *behaviour* axis — which of the four upstream
//!   components this is ([`Base`](ButtonVariant::Base),
//!   [`Metallic`](ButtonVariant::Metallic),
//!   [`Stateful`](ButtonVariant::Stateful),
//!   [`Magnetic`](ButtonVariant::Magnetic)).
//! - [`ButtonTone`] is the base's own `variant` prop — the fill treatment
//!   (`primary`/`secondary`/`ghost`/`outline`), renamed so the two axes cannot
//!   be confused at a call site.
//!
//! [`ButtonSize`] is the base's `size` prop verbatim.
//!
//! # The pointer-down rule the magnetic variant settles
//!
//! frust's hover link ends on every `Down`, while
//! [`PointerTracker`](crate::motion::PointerTracker) keeps tracking through one.
//! The two disagree, and a magnetic button is the first widget that has to say
//! which wins. **This widget's rule: a press keeps the magnetic offset.** The
//! pointer has not gone anywhere — dropping the pull on `Down` would snap the
//! control out from under the cursor at the exact moment the user is aiming at
//! it, which is the opposite of the effect's purpose. Concretely, the paint-time
//! [`sync_hovered`](crate::motion::PointerTracker::sync_hovered) correction is
//! skipped while the pointer is captured, and the offset returns to rest on
//! `Up`/`Cancel` (which the tracker already resets on) or on a move that leaves
//! the box.
//!
//! # Departures from the source
//!
//! - **No ripple.** `base.tsx`'s opt-in Material ripple is not ported; beUI's
//!   own default is off, and the catalog has no ripple vocabulary.
//! - **`ButtonLink` is not ported** — frust has no anchor element, and a
//!   navigation button is a callback here like any other.
//! - **The stateful button reserves its widest label** rather than morphing its
//!   own width. Upstream animates the button's width as the text swaps; that
//!   would mean a relayout every frame of the morph, so this port sizes the box
//!   once for the widest of the four state labels (plus the icon slot) and
//!   animates only inside it. The swap motion itself — the roll and the fade —
//!   is ported.
//! - **No blur filter anywhere.** Upstream's swap slots carry
//!   `filter: blur(...)`; frust's scene has no blur primitive, so every swap
//!   here is opacity + offset + scale only.
//! - **The metallic sheen does not skew.** Upstream's travelling highlight is a
//!   `-skew-x-12` band; this paints an unskewed gradient band with the same
//!   travel and timing.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedCallback,
    EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase, Role,
    SemanticsCtx, Size, Vec2, View, Widget, erase_callback, scene::arc_path,
};
use frust::{FrameTime, Theme};
use peniko::{ColorStop, Gradient, color::DynamicColor};

use crate::motion::{PointerTracker, Ramp};
use crate::press::{SpringScalar, inside, presses, stroke_outline};
use crate::style::{
    ACTIVE_CURSOR, DISABLED_OPACITY, GAP_MD, GAP_SM, HEIGHT_LG, HEIGHT_MD, HEIGHT_SM, HOVER_SCALE,
    HOVER_SOLID_ALPHA, HOVER_WASH_ALPHA, ICON_SIZE, PADDING_X_LG, PADDING_X_MD, PADDING_X_SM,
    PRESS_SCALE, RADIUS_CONTROL, RADIUS_ICON_BUTTON, SIZE_ICON_BUTTON, TEXT_BASE, TEXT_SM, TEXT_XS,
    disabled_tint, resolve_radius, scale_alpha, with_alpha,
};
use crate::text::{LabelRun, ThemeTextType, label_style};
use crate::tokens::color_scheme_light;
use crate::tokens::motion::{EASE_IN_OUT, EASE_OUT, SPRING_MOUSE, SPRING_PRESS, SPRING_SWAP};

// ---- Public axes -----------------------------------------------------------

/// Which of the upstream button components this is — the *behaviour* axis (see
/// the [module docs](self)).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// `button/base.tsx`: press scale, hover lift, tone and size.
    #[default]
    Base,
    /// `button/metallic.tsx`: a chrome rim with a drifting reflection and a
    /// highlight that sweeps across on hover. Paints its own surface, so
    /// [`ButtonTone`] is ignored (upstream forces `variant="ghost"` too).
    Metallic,
    /// `button/stateful.tsx`: idle → loading → success/error, the label and the
    /// icon slot swapping on `SPRING_SWAP`.
    Stateful,
    /// `button/magnetic.tsx`: the whole control translates toward the pointer
    /// and springs back on leave.
    Magnetic,
}

/// The base's own `variant` prop — the fill treatment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonTone {
    /// `bg-primary text-primary-foreground hover:bg-primary/90`.
    #[default]
    Primary,
    /// `border border-border bg-card text-foreground`.
    Secondary,
    /// Transparent, dimmed ink, `hover:bg-primary/5 hover:text-foreground`.
    Ghost,
    /// `border border-border bg-transparent hover:bg-primary/5`.
    Outline,
}

/// The base's `size` prop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// `h-8 px-3 text-xs gap-1.5 rounded-full`.
    Sm,
    /// `h-10 px-5 text-sm gap-2 rounded-full` — beUI's default.
    #[default]
    Md,
    /// `h-12 px-6 text-base gap-2 rounded-full`.
    Lg,
    /// `h-8 w-8 rounded-lg` — the one square, non-pill control.
    Icon,
}

impl ButtonSize {
    /// The control's height, in logical px.
    fn height(self) -> f64 {
        match self {
            ButtonSize::Sm => HEIGHT_SM,
            ButtonSize::Md => HEIGHT_MD,
            ButtonSize::Lg => HEIGHT_LG,
            ButtonSize::Icon => SIZE_ICON_BUTTON,
        }
    }

    /// Horizontal padding around the content — none on the fixed square.
    fn pad_x(self) -> f64 {
        match self {
            ButtonSize::Sm => PADDING_X_SM,
            ButtonSize::Md => PADDING_X_MD,
            ButtonSize::Lg => PADDING_X_LG,
            ButtonSize::Icon => 0.0,
        }
    }

    /// Gap between the icon slot and the label.
    fn gap(self) -> f64 {
        match self {
            ButtonSize::Sm => GAP_SM,
            _ => GAP_MD,
        }
    }

    /// Label type size.
    fn font_size(self) -> f64 {
        match self {
            ButtonSize::Sm | ButtonSize::Icon => TEXT_XS,
            ButtonSize::Md => TEXT_SM,
            ButtonSize::Lg => TEXT_BASE,
        }
    }

    /// The corner radius: a pill everywhere but the square icon size.
    fn radius(self) -> f64 {
        match self {
            ButtonSize::Icon => RADIUS_ICON_BUTTON,
            _ => RADIUS_CONTROL,
        }
    }
}

/// The [`ButtonVariant::Stateful`] machine's four states, upstream's
/// `ButtonState` verbatim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonState {
    /// The resting state: the button's own label, no state icon.
    #[default]
    Idle,
    /// In flight. Disables activation (`disabled={disabled || isBusy}`) and
    /// shows the spinner.
    Loading,
    /// Finished successfully — the check icon.
    Success,
    /// Failed — the cross icon.
    Error,
}

impl ButtonState {
    /// Whether this state suppresses activation. Upstream disables the button
    /// outright while loading.
    const fn is_busy(self) -> bool {
        matches!(self, ButtonState::Loading)
    }

    /// Whether this state draws an icon in the leading slot.
    const fn has_icon(self) -> bool {
        !matches!(self, ButtonState::Idle)
    }
}

// ---- Ported constants ------------------------------------------------------

/// The magnetic wrapper's pull strength as `MagneticButton` passes it:
/// `strength = 0.25` (`button/magnetic.tsx`), overriding the `Magnetic`
/// wrapper's own `0.35` default.
pub const MAGNETIC_STRENGTH: f64 = 0.25;

/// The width the stateful icon slot opens to: `width: "1.5rem"`
/// (`button/stateful.tsx`'s `ICON_VARIANTS`).
const ICON_SLOT_WIDTH: f64 = 24.0;

/// The type-scale role every state label's family resolves from at layout —
/// a control label. [`label_style`]'s own family is the unthemed base.
const LABEL_ROLE: ThemeTextType = ThemeTextType::LabelLarge;

/// How long a state's label takes to hand over. Upstream's exit is
/// `{ duration: 0.16, ease: EASE_OUT }`; the entrance is `SPRING_SWAP`, and the
/// two overlap.
const SWAP_EXIT: Duration = Duration::from_millis(160);

/// How far a swapping label travels vertically, as a fraction of its own
/// height: `y: "105%"` (`button/stateful.tsx`).
const SWAP_TRAVEL: f64 = 1.05;

/// The chrome rim's stops: `linear-gradient(105deg, …)` (`button/metallic.tsx`).
/// The angle is dropped — the band is painted horizontally (see the [module
/// docs](self)).
const METALLIC_STOPS: [(f32, Color); 9] = [
    (0.0, Color::from_rgb8(0x11, 0x11, 0x11)),
    (0.14, Color::from_rgb8(0x73, 0x73, 0x73)),
    (0.26, Color::from_rgb8(0xfa, 0xfa, 0xfa)),
    (0.38, Color::from_rgb8(0x52, 0x52, 0x52)),
    (0.50, Color::from_rgb8(0x0a, 0x0a, 0x0a)),
    (0.64, Color::from_rgb8(0xa3, 0xa3, 0xa3)),
    (0.75, Color::WHITE),
    (0.87, Color::from_rgb8(0x40, 0x40, 0x40)),
    (1.0, Color::from_rgb8(0x11, 0x11, 0x11)),
];

/// The inset of the metallic face inside its rim: `inset-[2px]`.
const METALLIC_RIM: f64 = 2.0;

/// The alpha the metallic face's hover surface is washed at:
/// `group-hover:bg-muted/40`.
const METALLIC_FACE_HOVER_ALPHA: f32 = 0.4;

/// The rim's drift period: `SILVER_DRIFT = { duration: 8, repeat: Infinity }`.
const METALLIC_DRIFT: Duration = Duration::from_secs(8);

/// How far the rim drifts, as a fraction of its own width:
/// `x: ["0%", "13%", "0%"]`.
const METALLIC_DRIFT_TRAVEL: f64 = 0.13;

/// The repaint cadence the drift asks for. The travel is ~13% of a control's
/// width over eight seconds — a couple of logical px per second — so 50ms steps
/// are already sub-pixel, far slower than the frame gate's own cosmetic cap.
const METALLIC_DRIFT_TICK: Duration = Duration::from_millis(50);

/// The hover highlight's sweep: `CHROME_SHIMMER = { duration: 2.4 }`.
const METALLIC_SHEEN: Duration = Duration::from_millis(2400);

/// The sheen band's width, as a fraction of the control's: `w-[52%]`.
const METALLIC_SHEEN_WIDTH: f64 = 0.52;

/// Peak opacity of the sheen band: `rgba(255,255,255,0.5)` at its midpoint,
/// under an `opacity-50` layer.
const METALLIC_SHEEN_ALPHA: f32 = 0.25;

/// One full turn of the loading spinner. Upstream is Tailwind's `animate-spin`,
/// whose period is 1s.
const SPINNER_PERIOD: Duration = Duration::from_secs(1);

/// The stroke width of the three state icons.
const ICON_STROKE: f64 = 2.0;

/// The spinner's open arc — a three-quarter sweep, the shape `Loader2` draws.
const SPINNER_SWEEP: f64 = std::f64::consts::PI * 1.5;

// ---- Resolved paint --------------------------------------------------------

/// A tone's resolved treatment at rest and while active.
///
/// `pub(crate)` because [`ButtonTone`] is the catalog's one fill ladder: the
/// action-swap button's `variant` prop is the same four treatments over the same
/// roles, so it names this type rather than growing a near-copy of it.
pub(crate) struct TonePaint {
    pub(crate) fill: Color,
    pub(crate) ink: Color,
    pub(crate) border: Option<Color>,
    /// `(fill, ink)` while hovered or pressed.
    pub(crate) active: (Color, Color),
}

/// The metallic variant's three resolved roles: the inner face at rest and on
/// hover, and the ink its label takes.
#[derive(Clone, Copy)]
struct MetallicPaint {
    face: Color,
    face_hover: Color,
    ink: Color,
}

impl MetallicPaint {
    fn resolve(theme: Option<&Theme>) -> Self {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(color_scheme_light().$role, |s| s.$role)
            };
        }
        Self {
            face: role!(surface),
            face_hover: scale_alpha(role!(surface_container_highest), METALLIC_FACE_HOVER_ALPHA),
            ink: role!(on_surface),
        }
    }
}

impl ButtonTone {
    pub(crate) fn resolve(self, theme: Option<&Theme>) -> TonePaint {
        let scheme = theme.map(Theme::scheme);
        macro_rules! role {
            ($role:ident) => {
                scheme.map_or(color_scheme_light().$role, |s| s.$role)
            };
        }
        let wash = with_alpha(role!(primary), HOVER_WASH_ALPHA);
        match self {
            ButtonTone::Primary => TonePaint {
                fill: role!(primary),
                ink: role!(on_primary),
                border: None,
                active: (
                    with_alpha(role!(primary), HOVER_SOLID_ALPHA),
                    role!(on_primary),
                ),
            },
            ButtonTone::Secondary => TonePaint {
                fill: role!(surface_container),
                ink: role!(on_surface),
                border: Some(role!(outline_variant)),
                // `hover:border-border` restates the resting border: nothing
                // changes on hover but the press scale.
                active: (role!(surface_container), role!(on_surface)),
            },
            ButtonTone::Ghost => TonePaint {
                fill: Color::TRANSPARENT,
                ink: role!(on_surface_variant),
                border: None,
                active: (wash, role!(on_surface)),
            },
            ButtonTone::Outline => TonePaint {
                fill: Color::TRANSPARENT,
                ink: role!(on_surface),
                border: Some(role!(outline_variant)),
                active: (wash, role!(on_surface)),
            },
        }
    }
}

// ---- View ------------------------------------------------------------------

/// A declarative beUI button.
pub struct ButtonView<State: 'static> {
    label: String,
    variant: ButtonVariant,
    tone: ButtonTone,
    size: ButtonSize,
    state: ButtonState,
    disabled: bool,
    paused: bool,
    strength: f64,
    loading_label: String,
    success_label: String,
    error_label: String,
    on_press: Rc<dyn Fn(&mut State)>,
}

/// A button labelled `label` that runs `on_press` when released inside its
/// bounds.
pub fn button<State: 'static>(
    label: impl Into<String>,
    on_press: impl Fn(&mut State) + 'static,
) -> ButtonView<State> {
    ButtonView {
        label: label.into(),
        variant: ButtonVariant::default(),
        tone: ButtonTone::default(),
        size: ButtonSize::default(),
        state: ButtonState::default(),
        disabled: false,
        paused: false,
        strength: MAGNETIC_STRENGTH,
        // `button/stateful.tsx`'s own prop defaults.
        loading_label: "Loading".to_string(),
        success_label: "Done".to_string(),
        error_label: "Try again".to_string(),
        on_press: Rc::new(on_press),
    }
}

impl<State: 'static> ButtonView<State> {
    /// Select the behaviour axis (default [`ButtonVariant::Base`]).
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Select the fill treatment (default [`ButtonTone::Primary`]). Ignored by
    /// [`ButtonVariant::Metallic`], which paints its own surface.
    pub fn tone(mut self, tone: ButtonTone) -> Self {
        self.tone = tone;
        self
    }

    /// Select the size (default [`ButtonSize::Md`]).
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Drive the [`ButtonVariant::Stateful`] machine. Ignored by every other
    /// variant.
    pub fn state(mut self, state: ButtonState) -> Self {
        self.state = state;
        self
    }

    /// Disable the button: suppresses the callback, blocks hover/cursor/focus
    /// claims, and dims every painted part by
    /// [`DISABLED_OPACITY`](crate::style::DISABLED_OPACITY).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stop [`ButtonVariant::Metallic`]'s travelling reflection while keeping
    /// the chrome rim — upstream's `paused` prop.
    pub fn paused(mut self, paused: bool) -> Self {
        self.paused = paused;
        self
    }

    /// The magnetic pull strength (default [`MAGNETIC_STRENGTH`]). Ignored by
    /// every variant but [`ButtonVariant::Magnetic`].
    pub fn strength(mut self, strength: f64) -> Self {
        self.strength = strength;
        self
    }

    /// The label shown in [`ButtonState::Loading`] (default `"Loading"`).
    pub fn loading_label(mut self, label: impl Into<String>) -> Self {
        self.loading_label = label.into();
        self
    }

    /// The label shown in [`ButtonState::Success`] (default `"Done"`).
    pub fn success_label(mut self, label: impl Into<String>) -> Self {
        self.success_label = label.into();
        self
    }

    /// The label shown in [`ButtonState::Error`] (default `"Try again"`).
    pub fn error_label(mut self, label: impl Into<String>) -> Self {
        self.error_label = label.into();
        self
    }

    /// The label the given state displays.
    fn label_for(&self, state: ButtonState) -> &str {
        match state {
            ButtonState::Idle => &self.label,
            ButtonState::Loading => &self.loading_label,
            ButtonState::Success => &self.success_label,
            ButtonState::Error => &self.error_label,
        }
    }
}

// ---- Widget ----------------------------------------------------------------

/// The retained widget for a [`ButtonView`].
pub struct ButtonWidget {
    /// The four state labels, in [`ButtonState`] order. A non-stateful button
    /// shapes only the first.
    labels: [LabelRun; 4],
    variant: ButtonVariant,
    tone: ButtonTone,
    size: ButtonSize,
    state: ButtonState,
    disabled: bool,
    paused: bool,
    strength: f64,
    on_press: ErasedCallback,

    hovered: bool,
    pressed: bool,
    captured: bool,

    /// The press/hover scale, on `SPRING_PRESS`.
    scale: SpringScalar,
    /// The magnetic pull, on `SPRING_MOUSE`, one scalar per axis.
    pull: (SpringScalar, SpringScalar),
    tracker: PointerTracker,

    /// The state the label is swapping *from*, and when the swap started.
    swap_from: Option<ButtonState>,
    swap_started: Option<FrameTime>,
    /// The frames the current metallic drift and sheen runs are timed from.
    drift_started: Option<FrameTime>,
    sheen_started: Option<FrameTime>,
    /// The content box measured at layout, reused while painting.
    content: Size,
}

impl ButtonWidget {
    /// The state whose label is currently on screen.
    fn shown_state(&self) -> ButtonState {
        if self.variant == ButtonVariant::Stateful {
            self.state
        } else {
            ButtonState::Idle
        }
    }

    /// Whether the button refuses activation — explicitly disabled, or busy.
    fn inert(&self) -> bool {
        self.disabled || (self.variant == ButtonVariant::Stateful && self.state.is_busy())
    }

    /// The label run for `state`.
    fn label(&self, state: ButtonState) -> &LabelRun {
        &self.labels[state as usize]
    }
}

impl<State: 'static> View<State> for ButtonView<State> {
    type Element = ButtonWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ButtonWidget {
        ButtonWidget {
            labels: [
                LabelRun::new(self.label.clone()),
                LabelRun::new(self.loading_label.clone()),
                LabelRun::new(self.success_label.clone()),
                LabelRun::new(self.error_label.clone()),
            ],
            variant: self.variant,
            tone: self.tone,
            size: self.size,
            state: self.state,
            disabled: self.disabled,
            paused: self.paused,
            strength: self.strength,
            on_press: erase_callback(&self.on_press),
            hovered: false,
            pressed: false,
            captured: false,
            scale: SpringScalar::new(1.0, Ramp::spring(SPRING_PRESS)),
            pull: (
                SpringScalar::new(0.0, Ramp::spring(SPRING_MOUSE)),
                SpringScalar::new(0.0, Ramp::spring(SPRING_MOUSE)),
            ),
            tracker: PointerTracker::new(),
            swap_from: None,
            swap_started: None,
            drift_started: None,
            sheen_started: None,
            content: Size::ZERO,
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ButtonWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_press = erase_callback(&self.on_press);
        let mut flags = ChangeFlags::NONE;

        for state in [
            ButtonState::Idle,
            ButtonState::Loading,
            ButtonState::Success,
            ButtonState::Error,
        ] {
            if prev.label_for(state) != self.label_for(state) {
                element.labels[state as usize].set_content(self.label_for(state).to_string());
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
        }
        if prev.variant != self.variant {
            element.variant = self.variant;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.tone != self.tone {
            element.tone = self.tone;
            flags |= ChangeFlags::PAINT;
        }
        if prev.size != self.size {
            element.size = self.size;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if prev.state != self.state {
            // The swap is staged here and timed from the frame it first paints.
            element.swap_from = Some(element.state);
            element.swap_started = None;
            element.state = self.state;
            flags |= ChangeFlags::PAINT;
        }
        if prev.paused != self.paused {
            element.paused = self.paused;
            flags |= ChangeFlags::PAINT;
        }
        if prev.strength != self.strength {
            element.strength = self.strength;
            flags |= ChangeFlags::PAINT;
        }
        if prev.disabled != self.disabled {
            element.disabled = self.disabled;
            flags |= ChangeFlags::PAINT;
            if self.disabled {
                element.pressed = false;
                element.captured = false;
                element.hovered = false;
                element.tracker.reset();
            }
        }
        flags
    }
}

impl Widget for ButtonWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let style = label_style(self.size.font_size());
        let mut content =
            self.labels[ButtonState::Idle as usize].layout_themed(ctx, &style, LABEL_ROLE);

        if self.variant == ButtonVariant::Stateful {
            // The box is sized for the widest state so the row never resizes
            // mid-morph — see the [module docs](self).
            for state in [
                ButtonState::Loading,
                ButtonState::Success,
                ButtonState::Error,
            ] {
                let size = self.labels[state as usize].layout_themed(ctx, &style, LABEL_ROLE);
                content.width = content.width.max(size.width);
                content.height = content.height.max(size.height);
            }
            content.width += ICON_SLOT_WIDTH + self.size.gap();
        }
        self.content = content;

        let height = self.size.height();
        if self.size == ButtonSize::Icon {
            return bc.constrain(Size::new(height, height));
        }
        bc.constrain(Size::new(content.width + self.size.pad_x() * 2.0, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Every token this pass paints with is resolved up front, so the theme
        // borrow ends before the frame requests below take `ctx` mutably.
        let theme = Theme::from_paint_ctx(ctx);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let tone = self.tone.resolve(theme);
        let metallic = MetallicPaint::resolve(theme);
        let now = ctx.frame_time();
        let size = ctx.size();
        let origin = ctx.origin();
        let disabled = self.disabled;

        // The authoritative hover read — except while this widget holds the
        // pointer, where the framework has already dropped the link and the
        // press is what the chrome should follow (see the [module docs](self)).
        if !disabled && !self.captured {
            self.hovered = ctx.is_hovered();
            self.tracker.sync_hovered(self.hovered);
        }
        self.tracker.set_size(size);

        // The press/hover scale. Upstream applies the hover lift only on a
        // hover-capable pointer and never under reduced motion; a pressed
        // control is always the deeper of the two.
        let target_scale = if disabled {
            1.0
        } else if self.pressed {
            PRESS_SCALE
        } else if self.hovered {
            HOVER_SCALE
        } else {
            1.0
        };
        let scale = if reduce {
            self.scale.jump_to(1.0);
            1.0
        } else {
            self.scale.set_target(target_scale);
            self.scale.advance(now)
        };

        // The magnetic pull.
        let pull = if self.variant == ButtonVariant::Magnetic && !reduce && !disabled {
            let magnetic = self.tracker.magnetic(self.strength);
            self.pull.0.set_target(magnetic.x);
            self.pull.1.set_target(magnetic.y);
            Vec2::new(self.pull.0.advance(now), self.pull.1.advance(now))
        } else {
            self.pull.0.jump_to(0.0);
            self.pull.1.jump_to(0.0);
            Vec2::ZERO
        };

        let centre = origin + Vec2::new(size.width / 2.0, size.height / 2.0);
        let transformed = pull != Vec2::ZERO || scale != 1.0;
        if transformed {
            scene.push_transform(
                Affine::translate(pull)
                    * Affine::translate(centre.to_vec2())
                    * Affine::scale(scale)
                    * Affine::translate(-centre.to_vec2()),
            );
        }

        let radius = resolve_radius(self.size.radius(), size.width, size.height);
        let ink = if self.variant == ButtonVariant::Metallic {
            self.paint_metallic(ctx, scene, metallic, reduce, now, origin, size, radius)
        } else {
            self.paint_surface(scene, &tone, disabled, origin, size, radius)
        };

        self.paint_content(scene, now, reduce, ink, origin, size);

        if transformed {
            scene.pop_transform();
        }

        let springs_running =
            self.scale.is_animating() || self.pull.0.is_animating() || self.pull.1.is_animating();
        if !reduce && springs_running {
            ctx.request_frame();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.inert() {
            return EventResult::Ignored;
        }
        let InputEvent::Pointer(p) = event else {
            return EventResult::Ignored;
        };
        let size = ctx.size();
        let magnetic = self.variant == ButtonVariant::Magnetic;

        match p.phase {
            PointerPhase::Down => {
                if !presses(p) || !inside(p.position, size) {
                    return EventResult::Ignored;
                }
                self.pressed = true;
                self.captured = true;
                if magnetic {
                    self.tracker.on_pointer(p, size);
                }
                ctx.capture_pointer();
                ctx.request_focus();
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Move => {
                let over = inside(p.position, size);
                if self.captured {
                    self.pressed = over;
                    if magnetic {
                        self.tracker.on_pointer(p, size);
                    }
                    ctx.set_cursor(ACTIVE_CURSOR);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if over {
                    ctx.claim_hover();
                    ctx.set_cursor(ACTIVE_CURSOR);
                }
                let mut changed = false;
                if magnetic {
                    changed = self.tracker.on_pointer(p, size);
                }
                if self.hovered != over {
                    self.hovered = over;
                    changed = true;
                }
                if changed {
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Up => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                if inside(p.position, size) {
                    (self.on_press)(ctx);
                }
                self.pressed = false;
                self.captured = false;
                self.tracker.on_pointer(p, size);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if !self.captured {
                    return EventResult::Ignored;
                }
                self.pressed = false;
                self.captured = false;
                self.tracker.on_pointer(p, size);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Button, |node| {
            node.set_label(self.label(self.shown_state()).content());
            if self.inert() {
                node.set_disabled();
            } else {
                node.add_action(Action::Click);
            }
        });
    }
}

impl ButtonWidget {
    /// Paint the tone's fill and border, returning the ink the content takes.
    fn paint_surface(
        &self,
        scene: &mut dyn PaintScene,
        paint: &TonePaint,
        disabled: bool,
        origin: Point,
        size: Size,
        radius: f64,
    ) -> Color {
        let active = !disabled && (self.hovered || self.pressed);
        let (fill, ink) = if active {
            paint.active
        } else {
            (paint.fill, paint.ink)
        };
        let fill = disabled_tint(fill, disabled, DISABLED_OPACITY);
        if fill.components[3] > 0.0 {
            scene.fill_rounded_rect(origin, size, radius, fill);
        }
        if let Some(border) = paint.border {
            stroke_outline(
                scene,
                origin,
                size,
                radius,
                disabled_tint(border, disabled, DISABLED_OPACITY),
            );
        }
        disabled_tint(ink, disabled, DISABLED_OPACITY)
    }

    /// Paint the chrome rim, its drifting reflection, the inner face and the
    /// hover sheen; returns the ink the content takes.
    #[allow(clippy::too_many_arguments)]
    fn paint_metallic(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        colors: MetallicPaint,
        reduce: bool,
        now: FrameTime,
        origin: Point,
        size: Size,
        radius: f64,
    ) -> Color {
        let still = self.paused || reduce;

        // The rim: the nine-stop chrome band, drifting horizontally.
        let drift = if still {
            self.drift_started = None;
            0.0
        } else {
            let started = *self.drift_started.get_or_insert(now);
            let elapsed = now.saturating_sub(started).as_secs_f64();
            let period = METALLIC_DRIFT.as_secs_f64();
            // `["0%", "13%", "0%"]` — out and back over one period.
            let phase = (elapsed % period) / period;
            let triangle = 1.0 - (2.0 * phase - 1.0).abs();
            triangle * METALLIC_DRIFT_TRAVEL * size.width
        };
        let band = Gradient::new_linear(
            Point::new(origin.x + drift, origin.y),
            Point::new(origin.x + drift + size.width, origin.y),
        )
        .with_stops(METALLIC_STOPS.map(|(offset, color)| ColorStop {
            offset,
            color: DynamicColor::from_alpha_color(color),
        }));
        scene.fill_rounded_rect_brush(origin, size, radius, &Brush::Gradient(band));

        // The face, lifting toward `muted` on hover (`group-hover:bg-muted/40`).
        let face_origin = Point::new(origin.x + METALLIC_RIM, origin.y + METALLIC_RIM);
        let face_size = Size::new(
            (size.width - METALLIC_RIM * 2.0).max(0.0),
            (size.height - METALLIC_RIM * 2.0).max(0.0),
        );
        let face_radius = (radius - METALLIC_RIM).max(0.0);
        let face = if self.hovered {
            colors.face_hover
        } else {
            colors.face
        };
        scene.fill_rounded_rect(
            face_origin,
            face_size,
            face_radius,
            disabled_tint(face, self.disabled, DISABLED_OPACITY),
        );

        // The hover sheen: one sweep per hover, left to right.
        if self.hovered && !still {
            let started = *self.sheen_started.get_or_insert(now);
            let elapsed = now.saturating_sub(started);
            let progress = Ramp::eased(METALLIC_SHEEN, EASE_IN_OUT).progress_clamped(elapsed);
            let band_width = (face_size.width * METALLIC_SHEEN_WIDTH).max(1.0);
            let travel = face_size.width + band_width * 2.0;
            let x = face_origin.x - band_width + progress * travel;
            let highlight = Gradient::new_linear(
                Point::new(x, face_origin.y),
                Point::new(x + band_width, face_origin.y),
            )
            .with_stops([
                (0.0f32, Color::TRANSPARENT),
                (0.5f32, with_alpha(Color::WHITE, METALLIC_SHEEN_ALPHA)),
                (1.0f32, Color::TRANSPARENT),
            ]);
            scene.push_clip_rounded(face_origin, face_size, face_radius);
            scene.fill_rect_brush(face_origin, face_size, &Brush::Gradient(highlight));
            scene.pop_clip();
            if elapsed < METALLIC_SHEEN {
                ctx.request_frame();
            }
        } else {
            self.sheen_started = None;
        }

        if !still {
            ctx.request_frame_paced_at(METALLIC_DRIFT_TICK);
        }
        disabled_tint(colors.ink, self.disabled, DISABLED_OPACITY)
    }

    /// Paint the icon slot and the label, staging the state swap.
    fn paint_content(
        &mut self,
        scene: &mut dyn PaintScene,
        now: FrameTime,
        reduce: bool,
        ink: Color,
        origin: Point,
        size: Size,
    ) {
        let shown = self.shown_state();
        if self.variant != ButtonVariant::Stateful {
            let label = self.label(shown);
            let label_size = label.size();
            let at = Point::new(
                origin.x + (size.width - label_size.width) / 2.0,
                origin.y + (size.height - label_size.height) / 2.0,
            );
            label.paint(at, ink, scene);
            return;
        }

        let elapsed = self.swap_from.map(|_| {
            let started = *self.swap_started.get_or_insert(now);
            now.saturating_sub(started)
        });
        let enter = Ramp::spring(SPRING_SWAP);
        let exit = Ramp::eased(SWAP_EXIT, EASE_OUT);
        let (enter_p, exit_p) = match (elapsed, reduce) {
            (None, _) | (_, true) => (1.0, 1.0),
            (Some(elapsed), false) => (
                enter.progress_clamped(elapsed),
                exit.progress_clamped(elapsed),
            ),
        };
        if let Some(elapsed) = elapsed
            && (reduce || (enter.is_settled(elapsed) && exit.is_settled(elapsed)))
        {
            self.swap_from = None;
            self.swap_started = None;
        }

        // The slot layout: an icon box the non-idle states fill, then the label,
        // both centred as one group inside the reserved content box.
        let label = self.label(shown);
        let label_size = label.size();
        let gap = self.size.gap();
        let slot = if shown.has_icon() {
            ICON_SLOT_WIDTH + gap
        } else {
            0.0
        };
        let group_width = slot + label_size.width;
        let left = origin.x + (size.width - group_width) / 2.0;
        let mid_y = origin.y + size.height / 2.0;

        if shown.has_icon() {
            let icon_centre = Point::new(left + ICON_SLOT_WIDTH / 2.0, mid_y);
            paint_state_icon(scene, shown, icon_centre, ink, enter_p, now, reduce);
        }
        let label_left = left + slot;
        let label_at = Point::new(label_left, mid_y - label_size.height / 2.0);

        // The incoming label rolls up into place; the outgoing one keeps rolling
        // out above it. Both are opacity + offset only — no blur.
        let enter_offset = (1.0 - enter_p) * label_size.height * SWAP_TRAVEL;
        scene.push_layer(label_at, label_size, enter_p.clamp(0.0, 1.0) as f32);
        label.paint(
            Point::new(label_at.x, label_at.y + enter_offset),
            ink,
            scene,
        );
        scene.pop_layer();

        if let Some(from) = self.swap_from
            && from != shown
        {
            let leaving = self.label(from);
            let leaving_size = leaving.size();
            let at = Point::new(
                label_left,
                mid_y - leaving_size.height / 2.0 - exit_p * leaving_size.height * SWAP_TRAVEL,
            );
            scene.push_layer(at, leaving_size, (1.0 - exit_p).clamp(0.0, 1.0) as f32);
            leaving.paint(at, ink, scene);
            scene.pop_layer();
        }
    }
}

/// Paint the stateful button's leading icon: a spinner while loading, a check on
/// success, a cross on error — all stroked in `ink`, scaled in by `enter`.
fn paint_state_icon(
    scene: &mut dyn PaintScene,
    state: ButtonState,
    centre: Point,
    ink: Color,
    enter: f64,
    now: FrameTime,
    reduce: bool,
) {
    // `scale: 0.7 → 1` on the way in (`ICON_VARIANTS`).
    let scale = 0.7 + 0.3 * enter.clamp(0.0, 1.0);
    let half = ICON_SIZE * scale / 2.0;
    let brush = Brush::Solid(ink);
    match state {
        ButtonState::Loading => {
            let start = if reduce {
                0.0
            } else {
                let period = SPINNER_PERIOD.as_secs_f64();
                (now.as_secs_f64() % period) / period * std::f64::consts::TAU
            };
            let path = arc_path(centre, half, start, SPINNER_SWEEP);
            scene.stroke_path(Point::ORIGIN, &path, ICON_STROKE, &brush);
        }
        ButtonState::Success => {
            let mut path = BezPath::new();
            path.move_to(Point::new(centre.x - half * 0.7, centre.y));
            path.line_to(Point::new(centre.x - half * 0.15, centre.y + half * 0.55));
            path.line_to(Point::new(centre.x + half * 0.7, centre.y - half * 0.55));
            scene.stroke_path(Point::ORIGIN, &path, ICON_STROKE, &brush);
        }
        ButtonState::Error => {
            let mut path = BezPath::new();
            path.move_to(Point::new(centre.x - half * 0.6, centre.y - half * 0.6));
            path.line_to(Point::new(centre.x + half * 0.6, centre.y + half * 0.6));
            path.move_to(Point::new(centre.x + half * 0.6, centre.y - half * 0.6));
            path.line_to(Point::new(centre.x - half * 0.6, centre.y + half * 0.6));
            scene.stroke_path(Point::ORIGIN, &path, ICON_STROKE, &brush);
        }
        ButtonState::Idle => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::scene::GlyphRun;
    use frust::authoring::text::TextContext;
    use frust::authoring::{PointerButton, PointerEvent};
    use std::any::Any;

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn build(view: &ButtonView<Counter>) -> ButtonWidget {
        let mut next_id = 0u64;
        View::<Counter>::build(view, &mut BuildCtx::new(&mut next_id))
    }

    fn rebuild(prev: &ButtonView<Counter>, next: &ButtonView<Counter>, w: &mut ButtonWidget) {
        let mut next_id = 0u64;
        View::<Counter>::rebuild(next, prev, w, &mut BuildCtx::new(&mut next_id));
    }

    fn layout(w: &mut ButtonWidget) -> Size {
        let mut text_ctx = TextContext::new();
        let mut ctx = LayoutCtx::with_resources(Some(&mut text_ctx as &mut dyn Any), None);
        w.layout(&mut ctx, &BoxConstraints::loose(Size::new(400.0, 200.0)))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ButtonWidget, state: &mut Counter, size: Size, event: &InputEvent) {
        let any_state: &mut dyn Any = state;
        let mut ctx = EventCtx::new(any_state, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    /// Records enough of the scene to assert what a variant painted.
    #[derive(Default)]
    struct Recorder {
        rrects: Vec<(Point, Size, f64, Color)>,
        gradients: usize,
        strokes: Vec<Color>,
        inks: Vec<Color>,
        alphas: Vec<f32>,
        transforms: Vec<Affine>,
    }

    impl PaintScene for Recorder {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
        fn fill_rounded_rect(&mut self, o: Point, s: Size, radius: f64, color: Color) {
            self.rrects.push((o, s, radius, color));
        }
        fn fill_rounded_rect_brush(&mut self, _o: Point, _s: Size, _r: f64, brush: &Brush) {
            if matches!(brush, Brush::Gradient(_)) {
                self.gradients += 1;
            }
        }
        fn fill_rect_brush(&mut self, _o: Point, _s: Size, brush: &Brush) {
            if matches!(brush, Brush::Gradient(_)) {
                self.gradients += 1;
            }
        }
        fn stroke_path(&mut self, _o: Point, _p: &BezPath, _w: f64, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.strokes.push(*color);
            }
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            if let Brush::Solid(color) = run.brush {
                self.inks.push(color);
            }
        }
        fn push_layer(&mut self, _o: Point, _s: Size, alpha: f32) {
            self.alphas.push(alpha);
        }
        fn push_transform(&mut self, transform: Affine) {
            self.transforms.push(transform);
        }
    }

    /// A real `RenderRoot` around one button — the only way a test can produce
    /// a genuine hover link, since `PaintCtx::is_hovered` (the authoritative
    /// read this widget self-corrects from) is seeded by the root and reads
    /// `false` on any synthetic context.
    struct HoverHarness {
        root: frust_core::RenderRoot<Counter, ButtonView<Counter>>,
        state: Counter,
        text_ctx: TextContext,
        size: Size,
    }

    impl HoverHarness {
        fn new(variant: ButtonVariant) -> Self {
            let mut root = frust_core::RenderRoot::new();
            root.set_theme(Box::new(crate::theme()));
            let mut harness = Self {
                root,
                state: Counter::default(),
                text_ctx: TextContext::new(),
                size: Size::ZERO,
            };
            let mut app = move |_s: &mut Counter| button::<Counter>("Go", |_| {}).variant(variant);
            harness.root.rebuild(&mut app, &mut harness.state);
            harness.size = harness.root.layout_with_text(
                Size::new(400.0, 200.0),
                &mut harness.text_ctx as &mut dyn Any,
            );
            harness
        }

        fn moved_to(&mut self, x: f64, y: f64) {
            self.root.event(
                &mut self.state,
                &InputEvent::Pointer(PointerEvent {
                    phase: PointerPhase::Move,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                }),
            );
        }

        fn painted(&mut self, ms: u64) -> Recorder {
            let mut rec = Recorder::default();
            self.root
                .paint(&mut rec, FrameTime::from_nanos(ms * 1_000_000));
            rec
        }

        /// The net horizontal translation the painted control carries — `0.0`
        /// when it pushed no transform at all, which is what a control at rest
        /// does.
        fn translation_x(&mut self, ms: u64) -> f64 {
            self.painted(ms)
                .transforms
                .iter()
                .map(|t| t.translation().x)
                .sum()
        }
    }

    /// Paint at `ms` on a synthetic clock, returning the recording and whether
    /// another frame was asked for.
    fn paint_at(
        w: &mut ButtonWidget,
        size: Size,
        theme: Option<&Theme>,
        ms: u64,
    ) -> (Recorder, bool) {
        let mut ctx =
            PaintCtx::for_test(Point::ORIGIN, size, FrameTime::from_nanos(ms * 1_000_000));
        if let Some(theme) = theme {
            ctx = ctx.with_theme(theme);
        }
        let mut rec = Recorder::default();
        w.paint(&mut ctx, &mut rec);
        (rec, ctx.needs_frame())
    }

    fn paint(w: &mut ButtonWidget, size: Size, theme: Option<&Theme>) -> Recorder {
        paint_at(w, size, theme, 0).0
    }

    fn reduced_theme() -> Theme {
        let mut theme = crate::theme();
        theme.motion.reduce_motion = true;
        theme
    }

    // ---- Sizes and construction --------------------------------------------

    /// Every registry variant of the slug builds and lays out — the shape of the
    /// acceptance check, and the guard against a variant that only exists in the
    /// enum.
    #[test]
    fn every_variant_and_size_lays_out_at_its_ladder_height() {
        for variant in [
            ButtonVariant::Base,
            ButtonVariant::Metallic,
            ButtonVariant::Stateful,
            ButtonVariant::Magnetic,
        ] {
            for (size, height) in [
                (ButtonSize::Sm, HEIGHT_SM),
                (ButtonSize::Md, HEIGHT_MD),
                (ButtonSize::Lg, HEIGHT_LG),
                (ButtonSize::Icon, SIZE_ICON_BUTTON),
            ] {
                let view = button::<Counter>("Ship", |_| {})
                    .variant(variant)
                    .size(size);
                let mut w = build(&view);
                let laid = layout(&mut w);
                assert_eq!(laid.height, height, "{variant:?}/{size:?}");
                if size == ButtonSize::Icon {
                    assert_eq!(laid.width, height, "the icon size is a square");
                }
            }
        }
    }

    /// The stateful button reserves the widest of its four labels plus the icon
    /// slot, so the box does not resize as the state changes — the documented
    /// departure from upstream's width morph.
    #[test]
    fn a_stateful_button_reserves_its_widest_label() {
        let short = button::<Counter>("Go", |_| {})
            .variant(ButtonVariant::Stateful)
            .loading_label("W")
            .success_label("W")
            .error_label("W");
        let wide = button::<Counter>("Go", |_| {})
            .variant(ButtonVariant::Stateful)
            .loading_label("A very much longer label")
            .success_label("W")
            .error_label("W");
        let mut narrow_widget = build(&short);
        let mut wide_widget = build(&wide);
        let narrow = layout(&mut narrow_widget);
        let broad = layout(&mut wide_widget);
        assert!(
            broad.width > narrow.width,
            "the widest state has to set the box: {broad:?} vs {narrow:?}"
        );

        // ...and the box is the same in every state, which is the point.
        let mut w = build(&wide);
        let idle = layout(&mut w);
        let next = button::<Counter>("Go", |_| {})
            .variant(ButtonVariant::Stateful)
            .loading_label("A very much longer label")
            .success_label("W")
            .error_label("W")
            .state(ButtonState::Success);
        rebuild(&wide, &next, &mut w);
        assert_eq!(layout(&mut w), idle);
    }

    /// A base button sizes itself around its label plus the size's padding.
    #[test]
    fn the_base_button_pads_its_label() {
        let view = button::<Counter>("Ship it", |_| {});
        let mut w = build(&view);
        let laid = layout(&mut w);
        let label = w.label(ButtonState::Idle).size();
        assert!((laid.width - (label.width + PADDING_X_MD * 2.0)).abs() < 0.01);
    }

    // ---- The press machine --------------------------------------------------

    #[test]
    fn down_then_up_inside_fires_once() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        assert!(w.pressed);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 1);
        assert!(!w.pressed);
    }

    #[test]
    fn a_release_outside_does_not_fire_and_a_cancel_never_does() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();

        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, size.width + 40.0, 5.0),
        );
        assert_eq!(state.presses, 0);

        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Cancel, 5.0, 5.0),
        );
        assert_eq!(state.presses, 0);
        assert!(!w.captured);
    }

    #[test]
    fn a_secondary_press_neither_captures_nor_fires() {
        let view = button::<Counter>("Go", |s| s.presses += 1);
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();
        let secondary = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: Point::new(5.0, 5.0),
            button: PointerButton::Secondary,
        });
        dispatch(&mut w, &mut state, size, &secondary);
        assert!(!w.pressed);
        assert!(!w.captured);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.presses, 0);
    }

    /// A disabled button takes no input at all, and a *loading* stateful button
    /// is disabled the same way (`disabled={disabled || isBusy}`).
    #[test]
    fn disabled_and_loading_both_refuse_every_pointer_event() {
        for view in [
            button::<Counter>("Go", |s| s.presses += 1).disabled(true),
            button::<Counter>("Go", |s| s.presses += 1)
                .variant(ButtonVariant::Stateful)
                .state(ButtonState::Loading),
        ] {
            let mut w = build(&view);
            let size = layout(&mut w);
            let mut state = Counter::default();
            dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
            dispatch(&mut w, &mut state, size, &ev(PointerPhase::Up, 5.0, 5.0));
            assert_eq!(state.presses, 0);
            assert!(!w.captured);
        }
    }

    #[test]
    fn a_hover_move_latches_the_flag_without_a_prior_down() {
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Move, 5.0, 5.0));
        assert!(w.hovered);
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, size.width + 20.0, 5.0),
        );
        assert!(!w.hovered);
    }

    // ---- The spring scalar --------------------------------------------------
    //
    // `SpringScalar`'s own contract (retarget/jump/advance) is exercised in
    // `crate::press`'s test module; the test below is button-behavioral, not
    // a leaf test.

    /// The press scale springs down on a press and back on release, and a
    /// reduced-motion theme flattens it to a constant 1.
    #[test]
    fn the_press_scale_springs_and_collapses_under_reduce_motion() {
        let theme = crate::theme();
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();

        paint_at(&mut w, size, Some(&theme), 0);
        assert_eq!(w.scale.value(), 1.0);
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        let (_, more) = paint_at(&mut w, size, Some(&theme), 0);
        assert!(more, "a running spring owes another frame");
        assert_eq!(w.scale.target(), PRESS_SCALE);
        paint_at(&mut w, size, Some(&theme), 30);
        assert!(w.scale.value() < 1.0 && w.scale.value() >= PRESS_SCALE);

        let reduced = reduced_theme();
        let (_, more) = paint_at(&mut w, size, Some(&reduced), 60);
        assert_eq!(w.scale.value(), 1.0);
        assert!(!more, "reduced motion asks for no frames");
    }

    // ---- The magnetic variant ------------------------------------------------

    /// The magnetic pull follows the pointer and returns to rest when it
    /// leaves — the acceptance criterion for the variant.
    #[test]
    fn the_magnetic_pull_follows_the_pointer_and_resets_on_leave() {
        let mut h = HoverHarness::new(ButtonVariant::Magnetic);
        assert_eq!(h.translation_x(0), 0.0, "at rest it pushes no transform");

        // A move to the right half pulls the control right, and keeps pulling
        // as the spring runs.
        let (right, mid_y) = (h.size.width - 1.0, h.size.height / 2.0);
        h.moved_to(right, mid_y);
        h.painted(0);
        let pulled = h.translation_x(400);
        assert!(pulled > 1.0, "pulled toward the cursor: {pulled}");

        // A move past the edge is a leave: with nothing claiming the link the
        // tracker recentres and the pull springs back to nothing.
        h.moved_to(h.size.width + 30.0, mid_y);
        h.painted(500);
        assert_eq!(
            h.translation_x(4_000),
            0.0,
            "the pull and the hover lift both settle back to rest"
        );
    }

    /// The mirror of the pull: a *non*-magnetic button never translates,
    /// however the pointer moves over it.
    #[test]
    fn a_base_button_never_translates_toward_the_pointer() {
        let mut h = HoverHarness::new(ButtonVariant::Base);
        h.moved_to(h.size.width - 1.0, h.size.height / 2.0);
        h.painted(0);
        // Only the hover lift is in play, and it scales about the centre rather
        // than displacing the control toward the cursor.
        let scaled = h.painted(400);
        assert_eq!(scaled.transforms.len(), 1);
        assert!(scaled.transforms[0].translation().x <= 0.0);
    }

    /// The documented Down-versus-hover rule: a press *keeps* the magnetic
    /// offset. `PaintCtx::is_hovered` reads false under a capture (the framework
    /// ends the hover link on `Down`), and the widget must not let that reset
    /// the pull out from under the pointer.
    #[test]
    fn a_press_keeps_the_magnetic_offset() {
        let theme = crate::theme();
        let view = button::<Counter>("Go", |_| {}).variant(ButtonVariant::Magnetic);
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();

        let edge = size.width - 1.0;
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, edge, size.height / 2.0),
        );
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Down, edge, size.height / 2.0),
        );
        assert!(w.captured);
        // A synthetic `PaintCtx` reports `is_hovered() == false`, exactly like a
        // real one does under a capture.
        paint_at(&mut w, size, Some(&theme), 0);
        assert!(
            w.tracker.hovered(),
            "the tracker must survive the press that ended the hover link"
        );
        assert!(w.pull.0.target() > 0.0);

        // Releasing does drop it.
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Up, edge, size.height / 2.0),
        );
        assert!(!w.tracker.hovered());
        paint_at(&mut w, size, Some(&theme), 10);
        assert_eq!(w.pull.0.target(), 0.0);
    }

    /// Reduced motion drops the pull entirely — upstream skips the effect under
    /// `useReducedMotion` rather than slowing it.
    #[test]
    fn reduced_motion_drops_the_magnetic_pull() {
        let reduced = reduced_theme();
        let view = button::<Counter>("Go", |_| {}).variant(ButtonVariant::Magnetic);
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();
        dispatch(
            &mut w,
            &mut state,
            size,
            &ev(PointerPhase::Move, size.width - 1.0, 5.0),
        );
        let (_, more) = paint_at(&mut w, size, Some(&reduced), 0);
        assert_eq!(w.pull.0.value(), 0.0);
        assert!(!more);
    }

    // ---- Tones ---------------------------------------------------------------

    #[test]
    fn each_tone_paints_its_own_fill_and_only_the_bordered_ones_stroke() {
        let theme = crate::theme();
        let scheme = theme.scheme();
        for (tone, fill, bordered) in [
            (ButtonTone::Primary, scheme.primary, false),
            (ButtonTone::Secondary, scheme.surface_container, true),
            (ButtonTone::Ghost, Color::TRANSPARENT, false),
            (ButtonTone::Outline, Color::TRANSPARENT, true),
        ] {
            let view = button::<Counter>("Go", |_| {}).tone(tone);
            let mut w = build(&view);
            let size = layout(&mut w);
            let rec = paint(&mut w, size, Some(&theme));
            if fill == Color::TRANSPARENT {
                assert!(rec.rrects.is_empty(), "{tone:?} fills nothing at rest");
            } else {
                assert_eq!(rec.rrects[0].3, fill, "{tone:?}");
            }
            assert_eq!(!rec.strokes.is_empty(), bordered, "{tone:?} border");
        }
    }

    #[test]
    fn an_unthemed_button_falls_back_to_the_light_table() {
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w);
        let rec = paint(&mut w, size, None);
        assert_eq!(rec.rrects[0].3, color_scheme_light().primary);
    }

    #[test]
    fn a_pressed_button_swaps_to_the_active_fill() {
        let theme = crate::theme();
        let view = button::<Counter>("Go", |_| {});
        let mut w = build(&view);
        let size = layout(&mut w);
        let mut state = Counter::default();
        dispatch(&mut w, &mut state, size, &ev(PointerPhase::Down, 5.0, 5.0));
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.rrects[0].3,
            with_alpha(theme.scheme().primary, HOVER_SOLID_ALPHA)
        );
    }

    #[test]
    fn disabled_dims_every_painted_part() {
        let theme = crate::theme();
        let view = button::<Counter>("Go", |_| {})
            .tone(ButtonTone::Outline)
            .disabled(true);
        let mut w = build(&view);
        let size = layout(&mut w);
        let rec = paint(&mut w, size, Some(&theme));
        assert_eq!(
            rec.strokes[0],
            disabled_tint(theme.scheme().outline_variant, true, DISABLED_OPACITY)
        );
        assert_eq!(
            rec.inks[0],
            disabled_tint(theme.scheme().on_surface, true, DISABLED_OPACITY)
        );
    }

    // ---- Metallic ------------------------------------------------------------

    /// The metallic variant paints its gradient rim and its inset face, and
    /// keeps asking for paced frames while the reflection drifts.
    #[test]
    fn the_metallic_variant_paints_a_rim_and_drifts_until_paused() {
        let theme = crate::theme();
        let view = button::<Counter>("Go", |_| {}).variant(ButtonVariant::Metallic);
        let mut w = build(&view);
        let size = layout(&mut w);

        let (rec, more) = paint_at(&mut w, size, Some(&theme), 0);
        assert_eq!(rec.gradients, 1, "the chrome rim");
        assert_eq!(rec.rrects.len(), 1, "the inset face");
        assert_eq!(rec.rrects[0].1.width, size.width - METALLIC_RIM * 2.0);
        assert!(more, "a perpetual drift owes frames");

        let paused = button::<Counter>("Go", |_| {})
            .variant(ButtonVariant::Metallic)
            .paused(true);
        rebuild(&view, &paused, &mut w);
        let (_, more) = paint_at(&mut w, size, Some(&theme), 100);
        assert!(!more, "paused stops the loop");

        // ...and so does reduced motion.
        let reduced = reduced_theme();
        let mut running = build(&view);
        let (_, more) = paint_at(&mut running, size, Some(&reduced), 100);
        assert!(!more);
    }

    /// The hover sheen sweeps once and then stops asking for transition frames.
    #[test]
    fn the_metallic_sheen_sweeps_only_while_hovered() {
        let mut h = HoverHarness::new(ButtonVariant::Metallic);
        assert_eq!(h.painted(0).gradients, 1, "the rim alone at rest");

        h.moved_to(5.0, h.size.height / 2.0);
        assert_eq!(h.painted(10).gradients, 2, "rim plus sheen band");

        // Leaving drops the sweep so the next hover starts a fresh one.
        h.moved_to(h.size.width + 10.0, h.size.height / 2.0);
        assert_eq!(h.painted(20).gradients, 1);
    }

    // ---- Stateful ------------------------------------------------------------

    /// The state machine: every transition stages a swap, the swap runs, and it
    /// clears itself once both ramps have settled.
    #[test]
    fn a_state_transition_stages_a_swap_that_settles() {
        let theme = crate::theme();
        let idle = button::<Counter>("Save", |_| {}).variant(ButtonVariant::Stateful);
        let mut w = build(&idle);
        let size = layout(&mut w);
        assert!(w.swap_from.is_none());

        let loading = button::<Counter>("Save", |_| {})
            .variant(ButtonVariant::Stateful)
            .state(ButtonState::Loading);
        rebuild(&idle, &loading, &mut w);
        assert_eq!(w.swap_from, Some(ButtonState::Idle));
        assert_eq!(w.state, ButtonState::Loading);

        let (rec, _) = paint_at(&mut w, size, Some(&theme), 0);
        assert_eq!(rec.alphas.len(), 2, "both labels are composited mid-swap");
        assert!(rec.alphas[0] < 0.01, "the entering label starts invisible");
        assert!(!rec.strokes.is_empty(), "the spinner is stroked");

        // Well past both ramps the swap has retired itself.
        paint_at(&mut w, size, Some(&theme), 3_000);
        assert!(w.swap_from.is_none());
        let (rec, _) = paint_at(&mut w, size, Some(&theme), 3_100);
        assert_eq!(rec.alphas.len(), 1);
        assert_eq!(rec.alphas[0], 1.0);
    }

    /// Reduced motion still performs the transition — it just lands on the frame
    /// it starts.
    #[test]
    fn a_reduced_motion_swap_completes_immediately() {
        let reduced = reduced_theme();
        let idle = button::<Counter>("Save", |_| {}).variant(ButtonVariant::Stateful);
        let mut w = build(&idle);
        let size = layout(&mut w);
        let error = button::<Counter>("Save", |_| {})
            .variant(ButtonVariant::Stateful)
            .state(ButtonState::Error);
        rebuild(&idle, &error, &mut w);
        let (rec, _) = paint_at(&mut w, size, Some(&reduced), 0);
        assert_eq!(rec.alphas.len(), 1, "no outgoing label to composite");
        assert_eq!(rec.alphas[0], 1.0);
        assert!(w.swap_from.is_none());
    }

    /// Each non-idle state draws its own icon; idle draws none.
    #[test]
    fn only_the_non_idle_states_draw_an_icon() {
        let theme = crate::theme();
        for (state, strokes) in [
            (ButtonState::Idle, 0),
            (ButtonState::Loading, 1),
            (ButtonState::Success, 1),
            (ButtonState::Error, 1),
        ] {
            let view = button::<Counter>("Save", |_| {})
                .variant(ButtonVariant::Stateful)
                .state(state);
            let mut w = build(&view);
            let size = layout(&mut w);
            let rec = paint(&mut w, size, Some(&theme));
            assert_eq!(rec.strokes.len(), strokes, "{state:?}");
        }
    }

    /// The accessible name follows the displayed state's label.
    #[test]
    fn semantics_names_the_state_currently_shown() {
        let idle = button::<Counter>("Save", |_| {}).variant(ButtonVariant::Stateful);
        let mut w = build(&idle);
        assert_eq!(w.label(w.shown_state()).content(), "Save");
        let success = button::<Counter>("Save", |_| {})
            .variant(ButtonVariant::Stateful)
            .state(ButtonState::Success);
        rebuild(&idle, &success, &mut w);
        assert_eq!(w.label(w.shown_state()).content(), "Done");
        assert!(!w.inert());
    }

    // `inside`/`presses` (the hit-test and pointer-admission leaf helpers) and
    // `LabelRun`/`label_style` (the shaped-run cache) each carry their own
    // leaf tests in `crate::press` / `crate::text` now.

    // ---- Typeface: the labels follow the live theme ------------------------

    use crate::text::typeface_probe::{
        assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    const PROBE_WINDOW: Size = Size::new(400.0, 200.0);

    /// A base button, which paints its idle run, over a stateful one showing
    /// its success label, which is shaped by the widest-state loop.
    fn probe_view(_: &mut ()) -> frust::FlexView<()> {
        frust::column().child(button::<()>("Save", |_| {})).child(
            button::<()>("Save", |_| {})
                .variant(ButtonVariant::Stateful)
                .state(ButtonState::Success),
        )
    }

    #[test]
    fn labels_paint_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the button labels", probe_view, PROBE_WINDOW);
    }

    #[test]
    fn labels_follow_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the button labels", probe_view, PROBE_WINDOW);
    }
}
