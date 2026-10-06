//! The Material 3 Expressive `Snackbar`: a transient, low-emphasis bottom
//! message with an optional single action and an optional close affordance.
//!
//! Ported from `material_3_expressive` v1.0.8's `M3ESnackbar`/
//! `M3ESnackbarTheme`/`M3ESnackbarHost` (MIT, © 2026 Paa Developments;
//! `tmp/material_3_expressive/lib/components/snackbar/{m3e_snackbar.dart,
//! styles/m3e_snackbar_theme.dart,components/m3e_snackbar_host.dart}`,
//! retrieved 2026-08-20).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Presentation: kept-mounted host, not a navigator page or modal
//!
//! A snackbar never blocks the app: input outside its own small bottom-anchored
//! box must keep reaching whatever is under it. That rules out both of this
//! crate's existing presentation seams — [`mod@crate::dialog`]/[`crate::sheet`]
//! push a transparent, input-gating navigator page, and
//! [`mod@crate::overlay::modal`] paints a swallow-everything scrim — neither of
//! which a *non-modal* transient message may use. This module is the tier's
//! first host with no scrim and no navigator page at all: [`snackbar_host`]
//! wraps the app's own root view (the [`overlay::mod@self`]-documented
//! "kept-mounted" mount contract, one level up — see [`SnackbarHostView`]'s own
//! doc for the exact wiring) and paints its transient bar as a bottom-anchored
//! sibling layer over that content, swallowing only its own small box.
//!
//! # Mount contract
//!
//! An app keeps exactly one [`SnackbarController`] in its `Component::State`
//! and mounts [`snackbar_host`] **once, wrapping the whole app root** — the
//! same position [`frust::overlay_host`]/[`frust::navigator`] take:
//!
//! ```no_run
//! use frust::{Component, View, any, text};
//! use frust_material::{SnackbarController, snackbar, snackbar_host};
//!
//! struct AppState {
//!     toasts: SnackbarController<AppState>,
//! }
//!
//! #[derive(Default)]
//! struct App;
//!
//! impl Component for App {
//!     type State = AppState;
//!
//!     fn init(&self) -> AppState {
//!         AppState { toasts: SnackbarController::new() }
//!     }
//!
//!     fn build(&self, state: &mut AppState) -> impl View<AppState> {
//!         any(snackbar_host(&state.toasts, text("home")))
//!     }
//! }
//!
//! frust::app!(App);
//! # fn main() {}
//! ```
//!
//! From anywhere the app already has `&State` (an `on_press` handler, a
//! background task's result), call [`SnackbarController::show`]:
//!
//! ```no_run
//! # use frust_material::{SnackbarController, snackbar};
//! # struct AppState { toasts: SnackbarController<AppState> }
//! fn on_save(state: &mut AppState) {
//!     state.toasts.show(
//!         snackbar("Saved")
//!             .action("Undo", |s: &mut AppState| { /* revert */ let _ = s; })
//!             .dismissible(),
//!     );
//! }
//! ```
//!
//! `show` only *records* the request — [`SnackbarHostWidget`] drains it at its
//! own next rebuild, the same recorded-op shape
//! [`frust::NavigatorController::push`] uses (see that type's own module docs).
//! Calling `show` is enough on its own: the host's paint-driven ramp/timer loop
//! (below) keeps requesting frames for the whole visible lifetime, so no
//! further app action is needed to carry the message through its entrance,
//! hold, and exit.
//!
//! # Queue/replace rule
//!
//! At most one message is ever visible; a second [`SnackbarController::show`]
//! while one is showing does not interrupt it. This crate's own upstream
//! reference has **no** queue at all — `M3ESnackbar.show` inserts a fresh,
//! independent `OverlayEntry` on every call, so two calls made close together
//! paint two overlapping bars rather than one after another. Rather than port
//! that gap, this host follows the well-established platform convention for a
//! transient bottom message — Android Material Components' `SnackbarManager`
//! (`com.google.android.material.snackbar`) and Flutter's
//! `ScaffoldMessenger.showSnackBar` both hold **one pending slot** behind the
//! currently-visible message:
//!
//! - Nothing currently showing → the new message shows immediately.
//! - One message showing, no pending message queued → the new message becomes
//!   the pending one, shown once the current message's whole lifecycle
//!   (entrance, hold, exit) finishes.
//! - One message showing **and** one already pending → the new message
//!   **replaces** the pending one; the discarded pending message's `on_action`
//!   never fires.
//!
//! A single `Option` slot ([`SnackbarController`]'s `pending` field) implements
//! this precisely: an assignment always replaces whatever was there, and the
//! host only ever promotes it once [`SnackbarHostWidget`] has nothing of its
//! own currently showing.
//!
//! # Motion and color
//!
//! Slide-up + fade entrance (`crate::tokens::MaterialMotion::MEDIUM_2`, 300ms)
//! and a matching, shorter exit (`SHORT_4`, 200ms), both
//! `MaterialMotion::EMPHASIZED`-eased — the upstream host's own
//! `AnimationController` timings (`m3e_snackbar_host.dart`'s `duration`/
//! `reverseDuration`), reused for both the slide offset and an opacity fade
//! this module adds on top (the upstream host only slides). Container
//! `inverseSurface`, message ink `onInverseSurface`, action ink
//! `inversePrimary`, elevation level 3, corner radius `shape.extra_small` —
//! all transcribed from `m3e_snackbar_theme.dart`'s `M3ESnackbarTheme.defaults`.
//!
//! # Width
//!
//! The bar is `min(host_width - 32, 600)` wide and horizontally centered — on
//! a phone that is full-width-minus-16dp-margins (`600` never binds); on a wide
//! viewport it caps at 600dp and centers with more than 16dp of margin either
//! side. This is `m3e_snackbar_theme.dart`'s own shape: a `maxWidth: 600`
//! container inside a `Positioned(left: 16, right: 16, ...)` slot, aligned
//! `bottomCenter` — a child narrower than its aligned box centers within it
//! rather than staying pinned to the 16dp edges.
//!
//! # Deliberate v1 simplifications
//!
//! - **Single-line message, no wrap or ellipsis.** Overlong text is clipped to
//!   the bar's own rounded rect rather than wrapping or truncating with `…` —
//!   a stated scope cut, not a defect.
//! - **No hover chrome on the action/close affordances**, only a pressed-state
//!   tint. `PaintCtx::is_hovered`/`EventCtx::claim_hover` report *path*
//!   membership for the **whole** host (`docs/CODE_STANDARDS.md`'s Interaction
//!   Semantics), and this host also wraps arbitrary app content — a hover
//!   claimed by a button deep inside that content would read as "hovered" here
//!   too, so a self-corrected hover flag on the action/close region would be
//!   wrong more often than right. [`mod@crate::chips`]'s own delete icon
//!   carries the same no-hover-chrome simplification for an analogous reason
//!   (a small icon affordance beside a labeled control).
//! - **No window-inset awareness.** The bottom margin is a fixed constant, not
//!   folded against `WindowInsets` (the on-screen-keyboard/gesture-bar
//!   clearance the upstream host reads from `MediaQuery`). A future revision
//!   can fold `LayoutCtx::window_insets`/`PaintCtx::window_insets` in
//!   additively.
//!
//! # Hit region
//!
//! Hit-testing (the initial `Down` classification and every subsequent
//! `to_local` call while captured) reads [`ActiveSnackbar::visual_bar_rect`]
//! — [`ActiveSnackbar::bar_rect`]'s resting position shifted by the same
//! vertical offset [`Widget::paint`] applies — never the raw resting rect. A
//! press during the ~200-300ms entrance/exit ramp is therefore tested against
//! where the bar is actually painted (including nowhere at all, at
//! `progress <= 0.0`, when nothing is drawn yet), the same "hit-testing
//! follows the painted position, not a paint-only transform" contract
//! [`mod@crate::overlay::modal`]'s slide entrance documents for its own
//! panel.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::text::{FontWeight, LineHeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback, EventCtx,
    EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerEvent, PointerPhase, Role,
    SemanticsCtx, View, Widget, any, erase_callback,
};
use frust::{AnimationController, IconData, Theme};
use kurbo::{Affine, Point, Rect, Size, Vec2};
use peniko::{Brush, Color};

use crate::icons;
use crate::press::presses;
use crate::tokens::MaterialMotion;

// ---- Theme-token constants (m3e_snackbar_theme.dart's `M3ESnackbarTheme.defaults`) --

/// The bar's minimum height, in logical px (`minHeight` default).
const MIN_HEIGHT: f64 = 48.0;
/// The bar's maximum width, in logical px (`maxWidth` default).
const MAX_WIDTH: f64 = 600.0;
/// Horizontal content padding inside the bar (`contentPadding` default).
const CONTENT_PAD_H: f64 = 16.0;
/// Vertical content padding inside the bar (`contentPadding` default).
const CONTENT_PAD_V: f64 = 8.0;
/// Horizontal margin from the host's edges to the bar (`overlayHorizontalInset`
/// default).
const H_INSET: f64 = 16.0;
/// Vertical margin from the host's bottom edge to the bar
/// (`overlayBottomInset` default).
const BOTTOM_INSET: f64 = 16.0;
/// Padding inside the action's own tap target (`actionPadding` default).
const ACTION_PAD_H: f64 = 8.0;
const ACTION_PAD_V: f64 = 4.0;
/// Gap between the message and the action (`actionGap` default).
const ACTION_GAP: f64 = 8.0;
/// The default auto-dismiss hold duration (`defaultDuration` default, 4s).
const DEFAULT_DURATION: Duration = Duration::from_millis(4000);

/// The close icon's side length, in logical px. Not part of the upstream
/// theme (`m3e_snackbar_theme.dart` has no close affordance at all — see the
/// module docs) — 18dp mirrors this catalog's own delete-icon convention
/// ([`mod@crate::chips`]'s `CHIP_ICON_SIZE`).
const CLOSE_ICON_SIZE: f64 = 18.0;
/// Gap between the action (or the message, with no action) and the close icon.
const CLOSE_GAP: f64 = 8.0;

/// Entrance duration — the upstream host's `AnimationController.duration`
/// (`m3e_snackbar_host.dart`).
const ENTER_DURATION: Duration = MaterialMotion::MEDIUM_2;
/// Exit duration — the upstream host's `AnimationController.reverseDuration`.
const EXIT_DURATION: Duration = MaterialMotion::SHORT_4;

/// Extra clearance below the bar's resting position for the fully-hidden
/// (entrance start / exit end) slide offset, beyond the bar's own height plus
/// [`BOTTOM_INSET`] — enough that the bar is never partially visible at
/// `progress == 0`. This module's own addition (the upstream host's `Offset(0,
/// 1.5)` tween is tuned to its specific `SlideTransition` box and does not
/// port literally); the exact value is cosmetic, not spec-sourced.
const HIDDEN_MARGIN: f64 = 8.0;

/// Message type-scale token (M3 `bodyMedium`; `crate::tokens::type_scale`'s
/// `body_medium` row).
const MESSAGE_SIZE: f32 = 14.0;
const MESSAGE_LINE_HEIGHT: f32 = 20.0;
const MESSAGE_LETTER_SPACING: f32 = 0.25;
/// Action type-scale token (M3 `labelLarge`).
const ACTION_SIZE: f32 = 14.0;
const ACTION_LINE_HEIGHT: f32 = 20.0;
const ACTION_LETTER_SPACING: f32 = 0.1;

/// Unthemed-fallback container fill (`colors.inverse_surface`, M3 baseline
/// light).
const FALLBACK_CONTAINER: Color = Color::from_rgb8(0x32, 0x2F, 0x35);
/// Unthemed-fallback message/close ink (`colors.inverse_on_surface`, M3
/// baseline light).
const FALLBACK_ON_CONTAINER: Color = Color::from_rgb8(0xF5, 0xEF, 0xF7);
/// Unthemed-fallback action ink (`colors.inverse_primary`, M3 baseline light).
const FALLBACK_ACTION: Color = Color::from_rgb8(0xD0, 0xBC, 0xFF);
/// Unthemed-fallback corner radius (`shape.extra_small`, 4dp).
const FALLBACK_RADIUS: f64 = 4.0;
/// Unthemed-fallback shadow geometry/alpha, M3 elevation level 3
/// (`crate::tokens::elevation()`'s `y_offset = dp/2+1`, `blur = dp` mapping —
/// the same fallback convention [`mod@crate::overlay`] uses).
const FALLBACK_SHADOW_BLUR: f64 = 6.0;
const FALLBACK_SHADOW_Y: f64 = 4.0;
const FALLBACK_SHADOW_ALPHA: f32 = 0.3;

/// A pressed-state tint's opacity, taking this catalog's one state-layer
/// source (`docs/WIDGETS_CODE_STANDARDS.md`'s Theming & Animation
/// Conventions).
use crate::interaction::PRESSED_OPACITY;

/// A small allowance below which a settling ramp is treated as fully at rest —
/// the same epsilon [`mod@crate::overlay::modal`] uses for its own ramp
/// settle check.
const PROGRESS_EPSILON: f64 = 1e-4;
/// How far a layer rect is inflated beyond the bar's own box so the
/// elevation shadow (which reaches outside it) isn't clipped by the fade
/// layer.
const SHADOW_SPILL: f64 = 24.0;

/// Return `color` with its alpha channel replaced by `alpha` (mirrors every
/// other module's own copy of this helper, e.g. [`mod@crate::dialog`]'s).
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// The message's resolved style: the `bodyMedium` token literals above, in
/// the live theme's `bodyMedium` family when a theme reaches layout (unthemed:
/// the platform system UI family).
fn message_style(theme: Option<&Theme>) -> TextStyle {
    let mut style = TextStyle {
        weight: FontWeight::REGULAR,
        letter_spacing: MESSAGE_LETTER_SPACING,
        line_height: LineHeight::Absolute(MESSAGE_LINE_HEIGHT),
        ..TextStyle::new(MESSAGE_SIZE, Color::BLACK)
    };
    if let Some(theme) = theme {
        style.family = theme.type_scale.body_medium.family.clone();
    }
    style
}

/// The action label's resolved style: the `labelLarge` token literals above,
/// in the live theme's `labelLarge` family when a theme reaches layout.
fn action_style(theme: Option<&Theme>) -> TextStyle {
    let mut style = TextStyle {
        weight: FontWeight::MEDIUM,
        letter_spacing: ACTION_LETTER_SPACING,
        line_height: LineHeight::Absolute(ACTION_LINE_HEIGHT),
        ..TextStyle::new(ACTION_SIZE, Color::BLACK)
    };
    if let Some(theme) = theme {
        style.family = theme.type_scale.label_large.family.clone();
    }
    style
}

// ---- Text shaping helper -------------------------------------------------

/// A lazily-shaped, paint-time-rebrushed text run — the same idiom
/// [`mod@crate::badge`]'s `LabelRun` uses, shared here by the message and the
/// action, each shaped in the [`TextStyle`] its caller resolves. The cache is
/// keyed on the content *and* that style, so a theme swap that changes the
/// family reshapes instead of serving the old face.
struct TextRun {
    content: String,
    /// The style the cached layout was shaped with.
    style: TextStyle,
    layout: Option<TextLayout>,
}

impl TextRun {
    fn new() -> Self {
        Self {
            content: String::new(),
            style: TextStyle::default(),
            layout: None,
        }
    }

    fn set_content(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_string();
            self.layout = None;
        }
    }

    /// Shape (or reuse) the run, returning its measured (unwrapped,
    /// single-line — see the module docs' v1 simplifications) size.
    fn shape(&mut self, ctx: &mut LayoutCtx, style: &TextStyle) -> Size {
        if self.style != *style {
            self.style = style.clone();
            self.layout = None;
        }
        if let Some(layout) = &self.layout {
            return layout.size();
        }
        let text_ctx = ctx.text_context::<TextContext>();
        let laid = text_ctx.layout(&self.content, &self.style, None);
        let size = laid.size();
        self.layout = Some(laid);
        size
    }

    fn paint(&self, origin: Point, color: Color, scene: &mut dyn PaintScene) {
        let Some(layout) = &self.layout else {
            return;
        };
        for mut run in layout.to_scene_runs(origin) {
            run.brush = Brush::Solid(color);
            scene.draw_glyph_run(run);
        }
    }
}

// ---- Message builder + controller ----------------------------------------

/// A view-held, typed action callback (erased on drain) — mirrors
/// [`mod@crate::dialog`]'s own `OnDismiss` alias of the same shape.
type OnAction<State> = Rc<dyn Fn(&mut State)>;

/// A declarative snackbar message. Built via [`snackbar`], shown via
/// [`SnackbarController::show`]. See the [module docs](self).
pub struct SnackbarMessage<State: 'static> {
    message: String,
    action_label: Option<String>,
    on_action: Option<OnAction<State>>,
    duration: Duration,
    dismissible: bool,
}

/// Start building a snackbar message with body text `message`. Chain
/// [`SnackbarMessage::action`]/[`SnackbarMessage::duration`]/
/// [`SnackbarMessage::dismissible`], then pass the result to
/// [`SnackbarController::show`].
pub fn snackbar<State: 'static>(message: impl Into<String>) -> SnackbarMessage<State> {
    SnackbarMessage {
        message: message.into(),
        action_label: None,
        on_action: None,
        duration: DEFAULT_DURATION,
        dismissible: false,
    }
}

impl<State: 'static> SnackbarMessage<State> {
    /// Attach a single action button, labeled `label`. Pressing it fires
    /// `on_action` **and** dismisses the message immediately — Flutter's own
    /// stock `SnackBarAction` behavior (`flutter/material/snack_bar.dart`'s
    /// `_SnackBarActionState._handlePressed`, which always closes the
    /// snackbar right after invoking its callback); this crate's own upstream
    /// reference has no action-press dismiss logic of its own to port (its
    /// `onAction` callback is the only thing it fires — see the module docs'
    /// Queue/replace rule for the parallel gap in *that* file), so this
    /// follows the wider platform convention instead.
    pub fn action<F: Fn(&mut State) + 'static>(
        mut self,
        label: impl Into<String>,
        on_action: F,
    ) -> Self {
        self.action_label = Some(label.into());
        self.on_action = Some(Rc::new(on_action));
        self
    }

    /// Override the auto-dismiss hold duration. [`DEFAULT_DURATION`] (4s)
    /// unless set.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Show a close (×) icon that dismisses the message without firing
    /// `on_action`. Off unless called — the upstream reference has no close
    /// affordance at all (see the module docs).
    pub fn dismissible(mut self) -> Self {
        self.dismissible = true;
        self
    }
}

/// The app-state handle to a [`snackbar_host`]: a cloneable single-slot queue
/// an app keeps in its `Component::State` and drives with
/// [`show`](Self::show). Every clone shares one slot (`Rc`), mirroring
/// [`frust::NavigatorController`]'s own recorded-op shape — a call here only
/// *records* the request; [`SnackbarHostWidget`] drains it at its next
/// rebuild. See the [module docs](self) for the full mount contract and the
/// queue/replace rule this slot implements.
pub struct SnackbarController<State: 'static> {
    pending: Rc<RefCell<Option<SnackbarMessage<State>>>>,
}

impl<State: 'static> Clone for SnackbarController<State> {
    fn clone(&self) -> Self {
        Self {
            pending: Rc::clone(&self.pending),
        }
    }
}

impl<State: 'static> Default for SnackbarController<State> {
    fn default() -> Self {
        Self::new()
    }
}

impl<State: 'static> SnackbarController<State> {
    /// A controller with nothing pending.
    pub fn new() -> Self {
        Self {
            pending: Rc::new(RefCell::new(None)),
        }
    }

    /// Request `message` be shown. See the [module docs](self)' Queue/replace
    /// rule for exactly what happens when one is already visible or already
    /// pending.
    pub fn show(&self, message: SnackbarMessage<State>) {
        *self.pending.borrow_mut() = Some(message);
    }

    /// Take whatever is pending, leaving the slot empty. Only
    /// [`SnackbarHostView::build`]/[`SnackbarHostView::rebuild`] call this —
    /// both run inside a `View` pass generic over `State`, the one place this
    /// crate's callback-erasure rule
    /// (`docs/CODE_STANDARDS.md`'s Type-erase idiom) allows touching a typed
    /// closure before it is erased into the non-generic retained widget.
    fn take_pending(&self) -> Option<SnackbarMessage<State>> {
        self.pending.borrow_mut().take()
    }
}

// ---- The erased, retained-widget-facing message shape --------------------

/// [`SnackbarMessage`] with its typed `on_action` erased into an
/// [`ErasedCallback`] — what [`SnackbarHostWidget`] (which is never generic
/// over `State`, per `frust-widgets::authoring`'s callback-erasure rule)
/// actually stores.
struct ErasedSnackbar {
    message: String,
    action_label: Option<String>,
    on_action: Option<ErasedCallback>,
    duration: Duration,
    dismissible: bool,
}

fn erase<State: 'static>(msg: SnackbarMessage<State>) -> ErasedSnackbar {
    ErasedSnackbar {
        message: msg.message,
        action_label: msg.action_label,
        on_action: msg.on_action.map(|f| erase_callback(&f)),
        duration: msg.duration,
        dismissible: msg.dismissible,
    }
}

// ---- The host --------------------------------------------------------------

/// A declarative snackbar host wrapping `app`. See the [module docs](self).
pub struct SnackbarHostView<State: 'static> {
    controller: SnackbarController<State>,
    app: AnyView<State>,
}

/// Wrap `app` (the app's own root view) in a kept-mounted snackbar host driven
/// by `controller`. See the [module docs](self) for the full mount contract.
pub fn snackbar_host<State: 'static, V: View<State>>(
    controller: &SnackbarController<State>,
    app: V,
) -> SnackbarHostView<State> {
    SnackbarHostView {
        controller: controller.clone(),
        app: any(app),
    }
}

impl<State: 'static> View<State> for SnackbarHostView<State> {
    type Element = SnackbarHostWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SnackbarHostWidget {
        let mut widget = SnackbarHostWidget {
            app: frust::authoring::build_child(&self.app, ctx),
            current: None,
        };
        if let Some(msg) = self.controller.take_pending() {
            widget.begin_show(erase(msg));
        }
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SnackbarHostWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags =
            frust::authoring::rebuild_child(&prev.app, &self.app, &mut element.app, ctx);
        if element.current.is_none()
            && let Some(msg) = self.controller.take_pending()
        {
            element.begin_show(erase(msg));
            flags |= ChangeFlags::LAYOUT;
        }
        flags
    }

    fn teardown(&self, element: &mut SnackbarHostWidget, ctx: &mut BuildCtx<'_>) {
        frust::authoring::teardown_child(&self.app, &mut element.app, ctx);
    }
}

/// Which part of the bar a captured press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarRegion {
    Action,
    Close,
    /// The bar's own background — swallows the press (never dismisses; a
    /// modal-panel-background precedent, see [`mod@crate::dialog`]'s own
    /// module docs), never reaches `app`.
    Background,
}

/// One message's full lifecycle state — entrance, hold, exit. Never generic
/// over `State`: `on_action` is already an [`ErasedCallback`] by the time this
/// exists (see [`erase`]).
struct ActiveSnackbar {
    message: String,
    message_run: TextRun,
    action_label: Option<String>,
    action_run: Option<TextRun>,
    on_action: Option<ErasedCallback>,
    dismissible: bool,
    hold: AnimationController,

    /// Overall slide/fade progress, `0.0` (fully hidden) to `1.0` (fully
    /// shown) — authoritative between ramps, updated from `ramp_value()`
    /// while one is running. Mirrors `OverlayModalWidget::progress`'s own
    /// shape (`overlay::modal`).
    progress: f64,
    ramp_from: f64,
    ramp_to: f64,
    /// This leg's own eased `0.0..=1.0` fraction; `ramp_value()` interpolates
    /// `ramp_from..ramp_to` by it.
    anim: AnimationController,
    exiting: bool,

    captured_region: Option<BarRegion>,
    action_pressed: bool,
    close_pressed: bool,

    // Geometry from the last `Widget::layout`, in host-local coordinates.
    bar_rect: Rect,
    message_origin: Point,
    /// `None` when there is no action.
    action_rect: Option<Rect>,
    /// `None` when not [`ActiveSnackbar::dismissible`].
    close_rect: Option<Rect>,
}

impl ActiveSnackbar {
    fn new(msg: ErasedSnackbar) -> Self {
        let mut message_run = TextRun::new();
        message_run.set_content(&msg.message);
        let mut action_run = msg.action_label.as_ref().map(|_| TextRun::new());
        if let (Some(run), Some(label)) = (action_run.as_mut(), msg.action_label.as_ref()) {
            run.set_content(label);
        }
        let mut hold = AnimationController::new(msg.duration);
        hold.forward();
        let mut active = Self {
            message: msg.message,
            message_run,
            action_label: msg.action_label,
            action_run,
            on_action: msg.on_action,
            dismissible: msg.dismissible,
            hold,
            progress: 0.0,
            ramp_from: 0.0,
            ramp_to: 0.0,
            anim: AnimationController::new(ENTER_DURATION),
            exiting: false,
            captured_region: None,
            action_pressed: false,
            close_pressed: false,
            bar_rect: Rect::ZERO,
            message_origin: Point::ZERO,
            action_rect: None,
            close_rect: None,
        };
        active.begin_ramp(1.0, false, ENTER_DURATION);
        active
    }

    /// Start a fresh leg toward `to` over `duration`, from wherever
    /// [`ActiveSnackbar::progress`] currently sits — so a dismiss triggered
    /// mid-entrance reverses smoothly rather than jumping. Mirrors
    /// `OverlayModalWidget::begin_ramp` (`overlay::modal`).
    fn begin_ramp(&mut self, to: f64, exiting: bool, duration: Duration) {
        self.ramp_from = self.progress;
        self.ramp_to = to;
        self.exiting = exiting;
        self.anim = AnimationController::new(duration).with_curve(MaterialMotion::EMPHASIZED);
        self.anim.forward();
    }

    fn ramp_value(&self) -> f64 {
        self.ramp_from + (self.ramp_to - self.ramp_from) * self.anim.value_clamped()
    }

    /// Advance the running ramp and, if not already exiting, the auto-dismiss
    /// hold timer — starting the exit ramp itself the moment the timer
    /// expires. Returns whether another frame is needed to keep this message
    /// alive.
    fn advance(&mut self, now: frust::FrameTime) -> bool {
        let mut needs_frame = false;
        if self.anim.is_animating() {
            if self.anim.advance(now) {
                needs_frame = true;
            }
            self.progress = self.ramp_value();
        }
        if !self.exiting {
            if self.hold.advance(now) {
                needs_frame = true;
            } else {
                self.begin_ramp(0.0, true, EXIT_DURATION);
                needs_frame = true;
            }
        }
        needs_frame
    }

    /// Whether the exit ramp has fully settled (message ready to be dropped).
    fn settled_exit(&self) -> bool {
        self.exiting && !self.anim.is_animating() && self.progress <= PROGRESS_EPSILON
    }

    /// Fire the action's `on_action` callback, if one is attached. No haptic
    /// hook of its own — unlike the button family, an action press here has
    /// no upstream-cited haptic signal to route (see the module docs).
    fn fire_action(&mut self, ctx: &mut EventCtx) {
        if let Some(cb) = self.on_action.as_mut() {
            cb(ctx);
        }
    }

    /// The vertical slide offset [`Widget::paint`] currently applies on top of
    /// [`Self::bar_rect`]'s resting position — `0.0` at rest, growing toward
    /// the bar's own [`HIDDEN_MARGIN`]-past-fully-hidden offset as `progress`
    /// falls during the entrance/exit ramp (mirrors `paint`'s own `dy`
    /// formula exactly, so [`Self::visual_bar_rect`] never drifts from what is
    /// actually drawn).
    fn visual_dy(&self) -> f64 {
        let hidden_offset = self.bar_rect.height() + BOTTOM_INSET + HIDDEN_MARGIN;
        hidden_offset * (1.0 - self.progress.clamp(0.0, 1.0))
    }

    /// [`Self::bar_rect`] shifted by [`Self::visual_dy`] — the bar's actual
    /// on-screen box for this frame. Both [`Widget::paint`] and every
    /// hit-testing entry point ([`begin_press`]/[`to_local`]) read this rather
    /// than the raw resting rect, so a press during the entrance/exit ramp is
    /// tested against where the bar visually is (fully or partially off its
    /// resting position, or off-screen entirely at `progress <= 0.0`) instead
    /// of lagging behind at its final position — see the module docs' Hit
    /// region note.
    fn visual_bar_rect(&self) -> Rect {
        self.bar_rect + Vec2::new(0.0, self.visual_dy())
    }
}

/// The retained widget for a [`SnackbarHostView`]. See the [module docs](self).
pub struct SnackbarHostWidget {
    app: ChildPod,
    current: Option<ActiveSnackbar>,
}

impl SnackbarHostWidget {
    fn begin_show(&mut self, msg: ErasedSnackbar) {
        self.current = Some(ActiveSnackbar::new(msg));
    }
}

fn finite_or_zero(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

impl Widget for SnackbarHostWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let area = Size::new(
            finite_or_zero(bc.max().width),
            finite_or_zero(bc.max().height),
        );
        self.app.layout_child(ctx, &BoxConstraints::tight(area));
        self.app.set_origin(Point::ZERO);

        if let Some(active) = &mut self.current {
            let theme = Theme::from_layout_ctx(ctx);
            let (message_style, action_style) = (message_style(theme), action_style(theme));
            let message_size = active.message_run.shape(ctx, &message_style);
            let action_content = active.action_run.as_mut().map(|run| {
                let size = run.shape(ctx, &action_style);
                Size::new(
                    size.width + ACTION_PAD_H * 2.0,
                    size.height + ACTION_PAD_V * 2.0,
                )
            });
            let close_content = active
                .dismissible
                .then_some(Size::new(CLOSE_ICON_SIZE, CLOSE_ICON_SIZE));

            let available = (area.width - 2.0 * H_INSET).max(0.0);
            let bar_width = available.min(MAX_WIDTH);

            let mut content_height = message_size.height;
            if let Some(a) = action_content {
                content_height = content_height.max(a.height);
            }
            if let Some(c) = close_content {
                content_height = content_height.max(c.height);
            }
            let bar_height = (content_height + CONTENT_PAD_V * 2.0).max(MIN_HEIGHT);

            let mut right_x = bar_width - CONTENT_PAD_H;
            active.close_rect = close_content.map(|c| {
                right_x -= c.width;
                let y = (bar_height - c.height) / 2.0;
                let rect = Rect::new(right_x, y, right_x + c.width, y + c.height);
                right_x -= CLOSE_GAP;
                rect
            });
            active.action_rect = action_content.map(|a| {
                right_x -= a.width;
                let y = (bar_height - a.height) / 2.0;
                let rect = Rect::new(right_x, y, right_x + a.width, y + a.height);
                right_x -= ACTION_GAP;
                rect
            });

            let message_y = (bar_height - message_size.height) / 2.0;
            active.message_origin = Point::new(CONTENT_PAD_H, message_y);

            let bar_size = Size::new(bar_width, bar_height);
            let bar_x = (area.width - bar_width) / 2.0;
            let bar_y = area.height - BOTTOM_INSET - bar_height;
            active.bar_rect = Rect::from_origin_size(Point::new(bar_x, bar_y), bar_size);
        }

        bc.constrain(area)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.app.paint_child(ctx, scene);

        let now = ctx.frame_time();
        let Some(active) = self.current.as_mut() else {
            return;
        };

        if active.advance(now) {
            ctx.request_frame();
        }
        if active.settled_exit() {
            self.current = None;
            // The next rebuild is what can promote a queued message — ask for
            // the frame that carries it (the same drain-gotcha `overlay::modal`
            // documents for its own settle-and-fire path).
            ctx.request_frame();
            return;
        }

        let active = self.current.as_mut().expect("checked above");
        let progress = active.progress.clamp(0.0, 1.0);
        if progress <= 0.0 {
            return;
        }

        let theme = Theme::from_paint_ctx(ctx);
        let bar_size = active.bar_rect.size();
        let origin = ctx.origin() + active.visual_bar_rect().origin().to_vec2();

        let radius = theme.map_or(FALLBACK_RADIUS, |t| t.shape.extra_small);
        let (blur, y_off, shadow_color) = theme.map_or(
            (
                FALLBACK_SHADOW_BLUR,
                FALLBACK_SHADOW_Y,
                with_alpha(Color::BLACK, FALLBACK_SHADOW_ALPHA),
            ),
            |t| {
                let spec = t.elevation.level3.shadow(t.brightness);
                (
                    spec.blur_std_dev,
                    spec.y_offset,
                    with_alpha(t.scheme().shadow, spec.color_alpha),
                )
            },
        );
        let container = theme.map_or(FALLBACK_CONTAINER, |t| t.scheme().inverse_surface);
        let on_container = theme.map_or(FALLBACK_ON_CONTAINER, |t| t.scheme().inverse_on_surface);
        let action_ink = theme.map_or(FALLBACK_ACTION, |t| t.scheme().inverse_primary);

        let layer_rect =
            Rect::from_origin_size(origin, bar_size).inflate(SHADOW_SPILL, SHADOW_SPILL);
        scene.push_layer(layer_rect.origin(), layer_rect.size(), progress as f32);

        scene.draw_shadow(
            Point::new(origin.x, origin.y + y_off),
            bar_size,
            radius,
            blur,
            shadow_color,
        );
        scene.fill_rounded_rect(origin, bar_size, radius, container);

        scene.push_clip_rounded(origin, bar_size, radius);
        active.message_run.paint(
            origin + active.message_origin.to_vec2(),
            on_container,
            scene,
        );

        if let Some(rect) = active.action_rect {
            if active.action_pressed {
                scene.fill_rounded_rect(
                    origin + rect.origin().to_vec2(),
                    rect.size(),
                    radius,
                    with_alpha(action_ink, PRESSED_OPACITY),
                );
            }
            if let Some(run) = &active.action_run {
                let text_origin =
                    origin + rect.origin().to_vec2() + Vec2::new(ACTION_PAD_H, ACTION_PAD_V);
                run.paint(text_origin, action_ink, scene);
            }
        }

        if let Some(rect) = active.close_rect {
            if active.close_pressed {
                scene.fill_rounded_rect(
                    origin + rect.origin().to_vec2(),
                    rect.size(),
                    rect.width() / 2.0,
                    with_alpha(on_container, PRESSED_OPACITY),
                );
            }
            let (base_path, design) = IconData::from(icons::CLOSE).resolve();
            let scale = if design > 0.0 {
                CLOSE_ICON_SIZE / design
            } else {
                1.0
            };
            let scaled = Affine::scale(scale) * base_path;
            let icon_origin = origin + rect.origin().to_vec2();
            scene.fill_path(icon_origin, &scaled, &Brush::Solid(on_container));
        }

        scene.pop_clip();
        scene.pop_layer();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() || event.is_focus_routed() {
            return frust::authoring::route_event_single(&mut self.app, ctx, event);
        }
        let InputEvent::Pointer(p) = event else {
            return frust::authoring::route_event_single(&mut self.app, ctx, event);
        };

        if let Some(active) = self.current.as_mut()
            && active.captured_region.is_some()
        {
            return handle_captured(active, ctx, p);
        }
        if self.app.is_active() {
            return frust::authoring::route_event_single(&mut self.app, ctx, event);
        }
        if p.phase == PointerPhase::Down
            && presses(p)
            && let Some(active) = self.current.as_mut()
            && active.visual_bar_rect().contains(p.position)
        {
            return begin_press(active, ctx, p.position);
        }
        frust::authoring::route_event_single(&mut self.app, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.app.semantics_child(ctx);
        if let Some(active) = &self.current {
            let message = active.message.clone();
            let action_label = active.action_label.clone();
            let dismissible = active.dismissible;
            ctx.push_container(
                Role::Alert,
                move |node| node.set_label(message.as_str()),
                move |ctx| {
                    if let Some(label) = &action_label {
                        ctx.push_node(Role::Button, |node| {
                            node.set_label(label.as_str());
                            node.add_action(Action::Click);
                        });
                    }
                    if dismissible {
                        ctx.push_node(Role::Button, |node| {
                            node.set_label("Dismiss");
                            node.add_action(Action::Click);
                        });
                    }
                },
            );
        }
    }

    frust::authoring::visit_children!(app);
}

/// The bar's own local box (relative to whichever origin [`to_local`] is
/// resolving positions against) — the hit region for [`BarRegion::Background`].
fn local_bar_rect(active: &ActiveSnackbar) -> Rect {
    Rect::from_origin_size(Point::ZERO, active.bar_rect.size())
}

/// Translate a host-local `position` into bar-local coordinates, against
/// [`ActiveSnackbar::visual_bar_rect`] rather than the raw resting
/// [`ActiveSnackbar::bar_rect`] — so `action_rect`/`close_rect` (already
/// bar-local, unaffected by the vertical slide) line up with a press landing
/// on the bar wherever it is currently painted, not where it will rest. Used
/// by both [`begin_press`] and [`handle_captured`], so a captured drag
/// interrupted by an auto-dismiss mid-gesture still tracks the bar sliding
/// out from under it.
fn to_local(active: &ActiveSnackbar, position: Point) -> Point {
    let rect = active.visual_bar_rect();
    Point::new(position.x - rect.x0, position.y - rect.y0)
}

/// A fresh `Down` inside [`ActiveSnackbar::visual_bar_rect`]: classify which
/// region it landed in and capture the gesture.
fn begin_press(active: &mut ActiveSnackbar, ctx: &mut EventCtx, position: Point) -> EventResult {
    let local = to_local(active, position);
    let region = if active.close_rect.is_some_and(|r| r.contains(local)) {
        BarRegion::Close
    } else if active.action_rect.is_some_and(|r| r.contains(local)) {
        BarRegion::Action
    } else {
        BarRegion::Background
    };
    active.captured_region = Some(region);
    match region {
        BarRegion::Action => active.action_pressed = true,
        BarRegion::Close => active.close_pressed = true,
        BarRegion::Background => {}
    }
    ctx.capture_pointer();
    ctx.request_redraw();
    EventResult::Handled
}

/// Route a `Move`/`Up`/`Cancel` for an already-captured region. Fires on
/// up-inside only (`docs/CODE_STANDARDS.md`'s Interaction Semantics); `Cancel`
/// never touches `ctx.state_mut` — it may only clear flags.
fn handle_captured(
    active: &mut ActiveSnackbar,
    ctx: &mut EventCtx,
    p: &PointerEvent,
) -> EventResult {
    let region = active
        .captured_region
        .expect("handle_captured only runs while captured");
    let local = to_local(active, p.position);
    let target = match region {
        BarRegion::Action => active.action_rect,
        BarRegion::Close => active.close_rect,
        BarRegion::Background => Some(local_bar_rect(active)),
    };
    let over = target.is_some_and(|r| r.contains(local));

    match p.phase {
        PointerPhase::Move => {
            match region {
                BarRegion::Action => {
                    if active.action_pressed != over {
                        active.action_pressed = over;
                        ctx.request_redraw();
                    }
                }
                BarRegion::Close => {
                    if active.close_pressed != over {
                        active.close_pressed = over;
                        ctx.request_redraw();
                    }
                }
                BarRegion::Background => {}
            }
            EventResult::Handled
        }
        PointerPhase::Up => {
            if over {
                match region {
                    BarRegion::Action => {
                        active.fire_action(ctx);
                        active.begin_ramp(0.0, true, EXIT_DURATION);
                    }
                    BarRegion::Close => {
                        active.begin_ramp(0.0, true, EXIT_DURATION);
                    }
                    BarRegion::Background => {}
                }
            }
            active.captured_region = None;
            active.action_pressed = false;
            active.close_pressed = false;
            ctx.request_redraw();
            EventResult::Handled
        }
        PointerPhase::Cancel => {
            active.captured_region = None;
            active.action_pressed = false;
            active.close_pressed = false;
            ctx.request_redraw();
            EventResult::Handled
        }
        PointerPhase::Down => EventResult::Handled,
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use frust::FrameTime;
    use frust::authoring::text::FontFamily;
    use frust::authoring::{Key, NamedKey, PointerButton};
    use frust_widgets::test_support::{RecordingScene, probe};

    use super::*;

    const WINDOW: Size = Size::new(400.0, 800.0);

    fn build<S: 'static>(view: &SnackbarHostView<S>) -> SnackbarHostWidget {
        let mut counter = 0u64;
        View::<S>::build(view, &mut BuildCtx::new(&mut counter))
    }

    fn layout(w: &mut SnackbarHostWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(WINDOW))
    }

    fn ft_ms(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn frame(w: &mut SnackbarHostWidget, ms: f64) -> RecordingScene {
        let mut rec = RecordingScene::default();
        let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
        rec
    }

    fn pointer(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch<S: 'static>(
        w: &mut SnackbarHostWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(&mut ctx, event)
    }

    // ---- show -> visible -> auto-dismiss timeline (frame clock) -----------

    #[test]
    fn show_before_build_is_visible_at_construction() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(0).into_any());
        let w = build(&view);
        assert!(w.current.is_some());
        assert_eq!(w.current.as_ref().unwrap().message, "hi");
        assert_eq!(w.current.as_ref().unwrap().progress, 0.0, "not yet ramped");
    }

    #[test]
    fn the_entrance_ramp_reaches_full_progress_within_its_own_duration() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        layout(&mut w);

        frame(&mut w, 0.0);
        frame(&mut w, 350.0); // past ENTER_DURATION (300ms)

        assert_eq!(w.current.as_ref().unwrap().progress, 1.0);
    }

    #[test]
    fn with_no_interaction_the_message_auto_dismisses_after_its_hold_duration() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi").duration(Duration::from_millis(1000)));
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        layout(&mut w);

        frame(&mut w, 0.0);
        frame(&mut w, 350.0);
        assert_eq!(w.current.as_ref().unwrap().progress, 1.0, "fully shown");

        // Past the 1000ms hold: the exit ramp starts.
        frame(&mut w, 1100.0);
        assert!(w.current.as_ref().unwrap().exiting, "hold expired");

        // Seed the fresh exit ramp, then let it run past EXIT_DURATION (200ms).
        frame(&mut w, 1150.0);
        frame(&mut w, 1400.0);

        assert!(w.current.is_none(), "settled exit drops the message");
    }

    #[test]
    fn an_action_press_fires_the_callback_and_dismisses_the_message() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("Saved").action("Undo", |s: &mut Vec<u32>| s.push(99)));
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        layout(&mut w);
        frame(&mut w, 0.0);
        frame(&mut w, 350.0); // fully shown

        let action_rect = w.current.as_ref().unwrap().action_rect.unwrap();
        let bar_origin = w.current.as_ref().unwrap().bar_rect.origin();
        let press_at = bar_origin + action_rect.center().to_vec2();

        let mut state: Vec<u32> = Vec::new();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, press_at.x, press_at.y),
        );
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer(PointerPhase::Up, press_at.x, press_at.y)
            ),
            EventResult::Handled
        );

        assert_eq!(state, vec![99], "on_action fired exactly once");
        assert!(
            w.current.as_ref().unwrap().exiting,
            "action press dismisses"
        );

        frame(&mut w, 380.0); // seed the fresh exit ramp
        frame(&mut w, 650.0); // past EXIT_DURATION
        assert!(w.current.is_none());
    }

    #[test]
    fn a_close_press_dismisses_without_firing_on_action() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(
            snackbar("Saved")
                .action("Undo", |s: &mut Vec<u32>| s.push(99))
                .dismissible(),
        );
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        layout(&mut w);
        frame(&mut w, 0.0);
        frame(&mut w, 350.0);

        let close_rect = w.current.as_ref().unwrap().close_rect.unwrap();
        let bar_origin = w.current.as_ref().unwrap().bar_rect.origin();
        let press_at = bar_origin + close_rect.center().to_vec2();

        let mut state: Vec<u32> = Vec::new();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, press_at.x, press_at.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, press_at.x, press_at.y),
        );

        assert!(state.is_empty(), "close never fires on_action");
        assert!(w.current.as_ref().unwrap().exiting);
    }

    #[test]
    fn a_cancel_while_captured_clears_press_state_without_touching_state() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("Saved").action("Undo", |s: &mut Vec<u32>| s.push(99)));
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        layout(&mut w);
        frame(&mut w, 0.0);
        frame(&mut w, 350.0);

        let action_rect = w.current.as_ref().unwrap().action_rect.unwrap();
        let bar_origin = w.current.as_ref().unwrap().bar_rect.origin();
        let press_at = bar_origin + action_rect.center().to_vec2();

        // Cancel is delivered over a throwaway `()` state at a structural
        // rebuild in production; a `()` state here proves the handler never
        // reaches for `ctx.state_mut::<Vec<u32>>()`.
        dispatch(
            &mut w,
            &mut Vec::<u32>::new(),
            &pointer(PointerPhase::Down, press_at.x, press_at.y),
        );
        let mut unit_state = ();
        let state_any: &mut dyn Any = &mut unit_state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, WINDOW);
        w.event(
            &mut ctx,
            &pointer(PointerPhase::Cancel, press_at.x, press_at.y),
        );

        assert!(
            !w.current.as_ref().unwrap().exiting,
            "a Cancel never dismisses"
        );
        assert!(w.current.as_ref().unwrap().captured_region.is_none());
    }

    // ---- Queue/replace ------------------------------------------------------

    #[test]
    fn a_second_show_before_the_first_drains_replaces_it() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("first"));
        controller.show(snackbar("second"));
        let view = snackbar_host(&controller, probe(0).into_any());
        let w = build(&view);
        assert_eq!(w.current.as_ref().unwrap().message, "second");
    }

    #[test]
    fn a_show_while_one_is_current_waits_for_it_to_fully_exit() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("first").duration(Duration::from_millis(100)));
        let view1 = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view1);
        layout(&mut w);
        assert_eq!(w.current.as_ref().unwrap().message, "first");

        // A show() while "first" is current must not replace it.
        controller.show(snackbar("second"));
        let view2 = snackbar_host(&controller, probe(0).into_any());
        let mut counter = 0u64;
        view2.rebuild(&view1, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(
            w.current.as_ref().unwrap().message,
            "first",
            "a pending show must not interrupt the visible message"
        );

        // Run "first"'s full lifecycle to completion (entrance, 100ms hold,
        // exit) via the frame clock alone.
        frame(&mut w, 0.0);
        frame(&mut w, 350.0); // entrance settles
        frame(&mut w, 500.0); // past the 100ms hold -> exit begins
        assert!(w.current.as_ref().unwrap().exiting);
        frame(&mut w, 550.0); // seed the fresh exit ramp
        frame(&mut w, 800.0); // past EXIT_DURATION
        assert!(w.current.is_none());

        // Only *now*, on the next rebuild, is "second" promoted.
        let view3 = snackbar_host(&controller, probe(0).into_any());
        view3.rebuild(&view2, &mut w, &mut BuildCtx::new(&mut counter));
        assert_eq!(w.current.as_ref().unwrap().message, "second");
    }

    // ---- Non-modal: events outside the snackbar still route ----------------

    #[test]
    fn a_press_outside_the_bar_still_reaches_app_content_beneath_it() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(7).into_any());
        let mut w = build(&view);
        layout(&mut w);

        // Near the top of the window — clearly outside the bottom-anchored bar.
        let mut state: Vec<u32> = Vec::new();
        let result = dispatch(&mut w, &mut state, &pointer(PointerPhase::Down, 10.0, 10.0));
        assert_eq!(result, EventResult::Handled);
        assert_eq!(
            state,
            vec![7],
            "app content beneath the bar still receives input"
        );
    }

    #[test]
    fn a_press_on_the_bar_s_own_background_is_swallowed_not_forwarded_to_app() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("a message with no action"));
        let view = snackbar_host(&controller, probe(7).into_any());
        let mut w = build(&view);
        layout(&mut w);
        // Settle the entrance ramp first: an un-advanced widget is still at
        // `progress == 0.0` (nothing painted yet), which is itself the ramp
        // window the "Hit region tracks the paint-time translation" tests
        // below cover — their whole point is that a press lands nowhere
        // during it.
        frame(&mut w, 0.0);
        frame(&mut w, 350.0); // fully shown

        let bar_rect = w.current.as_ref().unwrap().bar_rect;
        let press_at = Point::new(bar_rect.center().x, bar_rect.center().y);

        let mut state: Vec<u32> = Vec::new();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, press_at.x, press_at.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, press_at.x, press_at.y),
        );

        assert!(
            state.is_empty(),
            "the bar's own background swallows the press"
        );
        assert!(!w.current.as_ref().unwrap().exiting, "and does not dismiss");
    }

    #[test]
    fn a_focus_routed_key_event_is_routed_by_focus_not_claimed_by_the_bar() {
        // The host is non-modal: it never calls `EventCtx::request_focus`
        // itself, so a Key event (focus-routed, never hit-tested) is routed
        // exactly like any single-child wrapper's — to whichever descendant
        // holds focus, never intercepted here. With nothing focused (this
        // test's `Probe` never claims it) that is an `Ignored` no-op, not a
        // panic or a swallow.
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(3).into_any());
        let mut w = build(&view);
        layout(&mut w);

        let mut state: Vec<u32> = Vec::new();
        let key = InputEvent::Key(frust::authoring::KeyEvent {
            key: Key::Named(NamedKey::Escape),
            modifiers: Default::default(),
            repeat: false,
        });
        let result = dispatch(&mut w, &mut state, &key);
        assert_eq!(result, EventResult::Ignored);
        assert!(state.is_empty(), "nothing was focused, so nothing fired");
    }

    // ---- Hit region tracks the paint-time translation -----------------------

    /// How far `progress` must fall below `1.0` before the resting rect's own
    /// center point lands outside [`ActiveSnackbar::visual_bar_rect`] — derived
    /// from the same geometry [`ActiveSnackbar::visual_dy`] uses, so this stays
    /// correct regardless of the ramp's easing curve.
    fn miss_threshold(bar_rect: Rect) -> f64 {
        1.0 - (bar_rect.height() / 2.0) / (bar_rect.height() + BOTTOM_INSET + HIDDEN_MARGIN)
    }

    #[test]
    fn a_press_at_the_resting_rect_mid_entrance_is_not_captured_and_reaches_app_content() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(7).into_any());
        let mut w = build(&view);
        layout(&mut w);

        let bar_rect = w.current.as_ref().unwrap().bar_rect;
        let threshold = miss_threshold(bar_rect);

        // Seed the ramp, then sample it well inside ENTER_DURATION (300ms) —
        // progress is well below rest, so the bar is painted far from (or, on
        // the very first frame, not painted anywhere near) its resting box.
        frame(&mut w, 0.0);
        frame(&mut w, 60.0);
        let progress = w.current.as_ref().unwrap().progress;
        assert!(
            progress < threshold,
            "sanity: still well into the entrance ramp (progress={progress}, threshold={threshold})"
        );

        let press_at = bar_rect.center();
        let mut state: Vec<u32> = Vec::new();
        let result = dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, press_at.x, press_at.y),
        );

        assert_eq!(
            result,
            EventResult::Handled,
            "the press falls through to the probe beneath, which claims it"
        );
        assert_eq!(
            state,
            vec![7],
            "a press at the resting rect mid-entrance reaches app content, not the bar"
        );
        assert!(
            w.current.as_ref().unwrap().captured_region.is_none(),
            "the bar must not capture a press landing on its future, not current, position"
        );
        assert!(
            !w.current.as_ref().unwrap().action_pressed,
            "no sub-region latched a press either"
        );
    }

    #[test]
    fn a_press_at_the_resting_rect_during_the_exit_ramp_is_not_captured_and_reaches_app_content() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi").dismissible());
        let view = snackbar_host(&controller, probe(7).into_any());
        let mut w = build(&view);
        layout(&mut w);
        frame(&mut w, 0.0);
        frame(&mut w, 350.0); // fully shown

        let bar_rect = w.current.as_ref().unwrap().bar_rect;
        let close_rect = w.current.as_ref().unwrap().close_rect.unwrap();
        let close_at = bar_rect.origin() + close_rect.center().to_vec2();

        let mut state: Vec<u32> = Vec::new();
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, close_at.x, close_at.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Up, close_at.x, close_at.y),
        );
        assert!(
            w.current.as_ref().unwrap().exiting,
            "the close press starts the exit ramp"
        );

        // Seed the fresh exit ramp, then sample it well inside EXIT_DURATION
        // (200ms).
        frame(&mut w, 380.0);
        frame(&mut w, 450.0);
        let progress = w.current.as_ref().unwrap().progress;
        let threshold = miss_threshold(bar_rect);
        assert!(
            progress < threshold,
            "sanity: still well into the exit ramp (progress={progress}, threshold={threshold})"
        );

        let press_at = bar_rect.center();
        let result = dispatch(
            &mut w,
            &mut state,
            &pointer(PointerPhase::Down, press_at.x, press_at.y),
        );

        assert_eq!(
            result,
            EventResult::Handled,
            "the press falls through to the probe beneath, which claims it"
        );
        assert_eq!(
            state,
            vec![7],
            "a press at the resting rect during the exit ramp reaches app content, not the bar"
        );
        assert!(w.current.as_ref().unwrap().captured_region.is_none());
    }

    #[test]
    fn a_settled_press_still_hits_the_bar_action_and_close_as_before() {
        // A control against the two tests above: once the entrance ramp has
        // fully settled, the resting rect and the visual rect coincide again,
        // so ordinary presses on the action/close sub-rects keep working
        // exactly like `an_action_press_fires_the_callback_and_dismisses_the_message`
        // and `a_close_press_dismisses_without_firing_on_action` already cover
        // end-to-end — this only re-asserts the geometry those rely on.
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi").dismissible());
        let view = snackbar_host(&controller, probe(7).into_any());
        let mut w = build(&view);
        layout(&mut w);
        frame(&mut w, 0.0);
        frame(&mut w, 350.0); // fully shown

        let active = w.current.as_ref().unwrap();
        assert_eq!(active.progress, 1.0);
        assert_eq!(
            active.visual_bar_rect(),
            active.bar_rect,
            "at rest, the visual and resting rects coincide"
        );
    }

    // ---- Width behavior ------------------------------------------------------

    #[test]
    fn phone_width_is_full_width_minus_margins() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(360.0, 800.0)));
        assert_eq!(
            w.current.as_ref().unwrap().bar_rect.width(),
            360.0 - 2.0 * H_INSET
        );
    }

    // ---- Typeface: the runs' families follow the live theme ------------------

    /// A host showing a message with an action, built but not yet laid out.
    fn host_with_action() -> SnackbarHostWidget {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("Saved").action("Undo", |s: &mut Vec<u32>| s.push(99)));
        build(&snackbar_host(&controller, probe(0).into_any()))
    }

    /// A baseline theme whose `bodyMedium`/`labelLarge` roles name `message`/
    /// `action`.
    fn theme_with(message: FontFamily, action: FontFamily) -> Theme {
        let mut theme = crate::baseline();
        theme.type_scale.body_medium.family = message;
        theme.type_scale.label_large.family = action;
        theme
    }

    /// Lay `w` out against `tcx`, threading `theme` the way the render root
    /// does.
    fn layout_themed(w: &mut SnackbarHostWidget, tcx: &mut TextContext, theme: Option<&Theme>) {
        let mut ctx =
            LayoutCtx::with_resources(Some(tcx as &mut dyn Any), theme.map(|t| t as &dyn Any));
        w.layout(&mut ctx, &BoxConstraints::tight(WINDOW));
    }

    #[test]
    fn layout_takes_the_message_and_action_families_from_their_roles() {
        let mut w = host_with_action();
        let mut tcx = TextContext::new();
        let (message, action) = (
            FontFamily::named("Message Role Probe"),
            FontFamily::named("Action Role Probe"),
        );
        layout_themed(
            &mut w,
            &mut tcx,
            Some(&theme_with(message.clone(), action.clone())),
        );
        let active = w.current.as_ref().expect("a message is showing");
        assert_eq!(active.message_run.style.family, message, "bodyMedium");
        let action_run = active.action_run.as_ref().expect("an action is set");
        assert_eq!(action_run.style.family, action, "labelLarge");
        // Only the family is themed: the token literals stay.
        assert_eq!(active.message_run.style.size, MESSAGE_SIZE);
        assert_eq!(action_run.style.size, ACTION_SIZE);
    }

    #[test]
    fn without_a_theme_the_runs_keep_the_unthemed_styles() {
        assert_eq!(
            message_style(None),
            TextStyle {
                weight: FontWeight::REGULAR,
                letter_spacing: MESSAGE_LETTER_SPACING,
                line_height: LineHeight::Absolute(MESSAGE_LINE_HEIGHT),
                ..TextStyle::new(MESSAGE_SIZE, Color::BLACK)
            }
        );
        assert_eq!(
            action_style(None),
            TextStyle {
                weight: FontWeight::MEDIUM,
                letter_spacing: ACTION_LETTER_SPACING,
                line_height: LineHeight::Absolute(ACTION_LINE_HEIGHT),
                ..TextStyle::new(ACTION_SIZE, Color::BLACK)
            }
        );

        let mut w = host_with_action();
        let mut tcx = TextContext::new();
        layout_themed(&mut w, &mut tcx, None);
        let active = w.current.as_ref().expect("a message is showing");
        assert_eq!(active.message_run.style, message_style(None));
        let action_run = active.action_run.as_ref().expect("an action is set");
        assert_eq!(action_run.style, action_style(None));
    }

    #[test]
    fn a_theme_swap_reshapes_the_cached_runs() {
        let mut w = host_with_action();
        let mut tcx = TextContext::new();
        let probe_a = || FontFamily::named("Snackbar Swap Probe A");
        let probe_b = || FontFamily::named("Snackbar Swap Probe B");
        let first = theme_with(probe_a(), probe_a());
        layout_themed(&mut w, &mut tcx, Some(&first));

        // Control: the same theme again reuses both cached runs outright.
        let settled = tcx.shape_cache_stats();
        layout_themed(&mut w, &mut tcx, Some(&first));
        assert_eq!(
            tcx.shape_cache_stats(),
            settled,
            "an unchanged theme reshapes nothing"
        );

        layout_themed(&mut w, &mut tcx, Some(&theme_with(probe_b(), probe_b())));
        assert_eq!(
            tcx.shape_cache_stats().shapes,
            settled.shapes + 2,
            "a family swap must reshape the message and the action"
        );
    }

    #[test]
    fn wide_viewport_caps_at_max_width_and_centers() {
        let controller: SnackbarController<Vec<u32>> = SnackbarController::new();
        controller.show(snackbar("hi"));
        let view = snackbar_host(&controller, probe(0).into_any());
        let mut w = build(&view);
        let mut tcx = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(1200.0, 800.0)));
        let bar = w.current.as_ref().unwrap().bar_rect;
        assert_eq!(bar.width(), MAX_WIDTH);
        assert!(
            (bar.center().x - 600.0).abs() < 1e-9,
            "centered, not edge-pinned"
        );
    }
}
