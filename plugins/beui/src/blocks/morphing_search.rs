//! Ports beUI's `morphing-search` block — `components/motion/morphing-search.tsx`
//! (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `morphing-search`: *"Search field or compact icon that morphs
//! into a glass results surface, whether opened by click or keyboard
//! shortcut."*
//!
//! | upstream | here |
//! |---|---|
//! | trigger `h-12 w-72` / `size-12`, `rounded-xl bg-background/60` | [`SEARCH_HEIGHT`], [`SEARCH_TRIGGER_WIDTH`], [`SEARCH_RADIUS`] |
//! | shell `width: panelWidth`, `height: 48 + resultsHeight` | the two shell lanes ([`SEARCH_SHELL`]) |
//! | `SEARCH_MORPH` `{type: "spring", duration: 0.58, bounce: 0.22}` | [`SEARCH_SHELL`] (converted — see below) |
//! | `SEARCH_CLIP_TRANSITION` `{duration: 0.32, ease: EASE_OUT}` | [`SEARCH_CLIP`] |
//! | `collapsedContentClip` `inset(0 (panelWidth − anchorWidth) resultsHeight 0)` | the trigger's own box, as the clip's source rect |
//! | trigger chrome fade `{duration: 0.1, delay: 0.1 / 0.12}` | [`SEARCH_CHROME_FADE`], [`SEARCH_CHROME_DELAY`] |
//! | query row `h-12 … gap-2.5`, `px-3.5` (`px-4` icon-only) | [`SEARCH_HEIGHT`], [`SEARCH_GAP`], [`SEARCH_PADDING_X`] |
//! | `kbd … h-7 rounded-md border px-2 text-xs` reading `Esc` | [`SEARCH_KBD_LABEL`], [`SEARCH_KBD_HEIGHT`] |
//! | results `p-2`, `maxHeight: resultsHeight` | [`SEARCH_LIST_PADDING`], [`SEARCH_RESULTS_MAX_HEIGHT`] |
//! | row `rounded-lg px-3 py-2.5 gap-2.5`, title `text-sm font-medium`, description `text-xs` | [`SEARCH_ROW_*`](SEARCH_ROW_HEIGHT) |
//! | active `bg-foreground/5` on `SPRING_LAYOUT` | [`SEARCH_PILL_ALPHA`], [`crate::tokens::motion::SPRING_LAYOUT`] |
//! | filter — case-folded `includes` over title + description + keywords | [`morphing_search_matches`] |
//! | empty `px-3 py-8 text-center text-sm` | [`SEARCH_EMPTY_PADDING_Y`] |
//!
//! # The morph is two lanes, not a keyframed box
//!
//! The trigger and the open panel share their **top-left corner** — upstream
//! positions the portalled shell at the measured anchor's own `top`/`left` — so
//! the whole shell morph is a width and a height, and each is a
//! [`Lane`](crate::press::Lane) rather than a progress ramp. That is what makes
//! it retarget cleanly: the results list resizes the panel every time the query
//! narrows it, and a lane re-aimed mid-flight continues from what is on screen
//! instead of restarting or snapping (`crate::press`'s lane contract). A
//! progress-driven `from → to` lerp cannot do that — its `to` moving mid-run
//! jumps the painted rect.
//!
//! The **content** unfolds on its own tween, exactly as upstream splits them:
//! *"keep the spring on the shell, but unfold complex clip-path values with the
//! same progressive tween … so the content never snaps ahead"*. Here that is a
//! clip rect travelling from the trigger's box to the panel's over
//! [`SEARCH_CLIP`], which is the same rect `collapsedContentClip` describes as
//! an inset.
//!
//! # The spring conversion
//!
//! `SEARCH_MORPH` is authored as Motion's `{duration, bounce}` pair, which this
//! catalog's [`Ramp`] does not take — [`crate::tokens::motion`]'s springs are
//! mass/stiffness/damping triples. [`SEARCH_SHELL`] is the converted
//! equivalent: Motion defines `dampingRatio = 1 − bounce` (`ζ = 0.78`), and the
//! remaining freedom is fixed by making the catalog's own settle time
//! ([`Ramp::settle`], `−ln(ε)/ζω₀` at `ε = 0.001`) land on upstream's `0.58s`,
//! which gives `ω₀ ≈ 15.25 rad/s` and, at unit mass, `k = ω₀² ≈ 233` with
//! `c = 2ζω₀ ≈ 23.8`. Approximate by construction — Motion's own perceptual
//! duration is not this settle definition — and marked as such rather than
//! presented as a transcribed constant.
//!
//! # Shape of the port
//!
//! Two halves, the division [`crate::components::combobox`] and
//! [`crate::components::context_menu`] both take:
//!
//! * [`morphing_search_trigger`] — the closed affordance, publishing its own
//!   window rect into an [`OverlayAnchor`] and reporting a press as an open
//!   request. Its chrome fades out as the panel takes over, upstream's
//!   opacity-to-zero overlay.
//! * [`morphing_search`] — the panel, mounted through
//!   [`crate::overlay::anchored`] at the trigger's own top-left corner, morphing
//!   out of it.
//!
//! The **query is controlled** and the **highlight is the widget's**, the same
//! split [`crate::blocks::command_palette`] documents, and the keyboard contract
//! is the same three intercepted keys with Escape left for the host.
//!
//! # Degradations against the web original
//!
//! - **The panel hugs its results instead of holding a fixed frame.** Upstream
//!   sizes the shell at `48 + resultsHeight` whatever the list holds, so a
//!   single result leaves a tall empty surface. Here the panel is its content,
//!   capped at [`SEARCH_RESULTS_MAX_HEIGHT`] (upstream's `288`) — which is also
//!   what gives the mid-flight retarget above something to retarget *to*.
//! - **The panel's width is capped, not fitted to the viewport's trailing
//!   edge.** Upstream computes `min(448, innerWidth − left − 16)`; here the
//!   width is [`SEARCH_MAX_WIDTH`] against the area, and
//!   [`crate::overlay::place`]'s clamp keeps the placed panel on screen.
//! - **No backdrop blur and no glass wash.** `bg-background/90 backdrop-blur-xl`
//!   has no `PaintScene` primitive; the panel takes the catalog's own opaque
//!   panel chrome, the call every overlay in this catalog makes.
//! - **No focus trap.** Upstream cycles Tab inside the dialog; there is no
//!   tab-order API to close a ring with (the gap
//!   [`crate::overlay::anchored`] records).
//! - **No auto-focus on open, and the shortcut is the app's.** Upstream focuses
//!   the field on the frame it opens and installs a `window` `keydown` listener
//!   for its `f` shortcut. A widget here has neither seam: the first press on
//!   the panel claims focus, and the shortcut is the app's own binding — the
//!   trigger only *draws* the key cap ([`MorphingSearchTriggerView::shortcut`]).
//!   [`crate::blocks::command_palette::CommandPaletteController`] is the shared
//!   controller shape for that binding.
//! - **No icons on the rows.** `MorphingSearchItem.icon` is a lucide component;
//!   the catalog has no icon vocabulary, so a result is its title and its
//!   optional description.
//! - **A longer list clips rather than scrolls**, the cap-and-clip
//!   [`crate::components::combobox`] documents for the same seam reason.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color, ErasedArgCallback,
    ErasedCallback, EventCtx, EventResult, InputEvent, Key, KeyEvent, LayoutCtx, NamedKey,
    PaintCtx, PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2,
    View, Widget, any, build_child, erase_callback, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child,
    text::{FontWeight, TextStyle},
    visit_children,
};
use frust::{FrameTime, SpringDescription, Theme, text_input};

use crate::blocks::command_palette::draw_search;
use crate::components::popover::{
    PanelChrome, lerp, morph_rect, paint_panel, paint_panel_hairline, resolve_panel,
};
use crate::motion::{Presence, PresencePhase, Ramp};
use crate::overlay::{
    AnchoredOverlayView, AnchoredOverlayWidget, OverlayAlign, OverlayAnchor, OverlayPlacement,
    OverlaySide, anchored,
};
use crate::press::{Lane, inside, is_activation_key, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::{EASE_OUT, SPRING_LAYOUT};

// ---- Metrics ---------------------------------------------------------------

/// `h-12` — the trigger's height, and the panel's query-row height, in logical
/// px. Also `size-12`'s side, so the icon-only trigger is square.
pub const SEARCH_HEIGHT: f64 = 48.0;

/// `w-72` — the full trigger's width, in logical px.
pub const SEARCH_TRIGGER_WIDTH: f64 = 288.0;

/// `rounded-xl` — the radius the trigger and the panel share, held constant
/// through the morph, in logical px.
pub const SEARCH_RADIUS: f64 = style::RADIUS_XL;

/// `min(448, …)` — the open panel's width cap, in logical px.
pub const SEARCH_MAX_WIDTH: f64 = 448.0;

/// `min(288, …)` — the results list's height cap, in logical px.
pub const SEARCH_RESULTS_MAX_HEIGHT: f64 = 288.0;

/// `px-3.5` — the query row's horizontal padding, in logical px.
pub const SEARCH_PADDING_X: f64 = 14.0;

/// `px-4` — the icon-only trigger's horizontal padding, in logical px.
pub const SEARCH_PADDING_X_ICON: f64 = 16.0;

/// `gap-2.5` — the gap between the search mark, the field and the key cap, in
/// logical px.
pub const SEARCH_GAP: f64 = 10.0;

/// `p-2` — the results list's padding, in logical px.
pub const SEARCH_LIST_PADDING: f64 = 8.0;

/// `px-3` — a result row's horizontal padding, in logical px.
pub const SEARCH_ROW_PADDING_X: f64 = 12.0;

/// `py-2.5` — a result row's vertical padding, in logical px.
pub const SEARCH_ROW_PADDING_Y: f64 = 10.0;

/// `rounded-lg` — a result row's (and the active pill's) radius, in logical px.
pub const SEARCH_ROW_RADIUS: f64 = style::RADIUS_LG;

/// A title-only row's height, in logical px (`py-2.5` around a `text-sm` line).
pub const SEARCH_ROW_HEIGHT: f64 = SEARCH_ROW_PADDING_Y * 2.0 + 20.0;

/// The extra height a row's `text-xs` description adds, in logical px.
pub const SEARCH_ROW_DESCRIPTION_HEIGHT: f64 = 16.0;

/// `py-8` — the empty state's vertical padding, in logical px.
pub const SEARCH_EMPTY_PADDING_Y: f64 = 32.0;

/// The empty state's row height, in logical px.
pub const SEARCH_EMPTY_HEIGHT: f64 = SEARCH_EMPTY_PADDING_Y * 2.0 + 20.0;

/// `h-7` — the `Esc` key cap's height, in logical px.
pub const SEARCH_KBD_HEIGHT: f64 = 28.0;

/// `px-2` — the key cap's horizontal padding, in logical px.
pub const SEARCH_KBD_PADDING_X: f64 = 8.0;

/// The text on the panel's trailing key cap.
pub const SEARCH_KBD_LABEL: &str = "Esc";

/// `bg-foreground/5` — the active row pill's alpha over the ink role.
pub const SEARCH_PILL_ALPHA: f32 = 0.05;

/// `bg-background/60` — the closed trigger's fill alpha over the surface role.
pub const SEARCH_TRIGGER_FILL_ALPHA: f32 = 0.60;

// ---- Motion ----------------------------------------------------------------

/// `SEARCH_MORPH` — the shell's morph spring, converted from Motion's
/// `{duration: 0.58, bounce: 0.22}` pair. **Community-approximate**: see the
/// [module docs](self)' conversion note for the arithmetic and its limits.
pub const SEARCH_SHELL: SpringDescription = SpringDescription {
    mass: 1.0,
    stiffness: 233.0,
    damping: 23.8,
};

/// `SEARCH_CLIP_TRANSITION.duration` — how long the content takes to unfold,
/// and to fold back.
pub const SEARCH_CLIP: Duration = Duration::from_millis(320);

/// How long the closed trigger's own chrome takes to fade out (and back in).
pub const SEARCH_CHROME_FADE: Duration = Duration::from_millis(100);

/// How long that fade waits before it starts (`delay: 0.1` opening, `0.12`
/// closing — the opening figure is kept for both).
pub const SEARCH_CHROME_DELAY: Duration = Duration::from_millis(100);

// ---- Items -----------------------------------------------------------------

/// One search result — upstream's `MorphingSearchItem`, minus the icon the
/// [module docs](self) record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MorphingSearchItem {
    title: String,
    description: Option<String>,
    keywords: Vec<String>,
}

/// A result showing `title`.
pub fn morphing_search_item(title: impl Into<String>) -> MorphingSearchItem {
    MorphingSearchItem {
        title: title.into(),
        description: None,
        keywords: Vec::new(),
    }
}

impl MorphingSearchItem {
    /// Set the second line under the title (`description`).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add search keywords the filter also matches against (`keywords`).
    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// This result's title.
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// Whether `item` matches `query` — upstream's own filter: the title, the
/// description and every keyword joined with spaces, case-folded, tested with
/// `includes`.
///
/// Deliberately a **substring** test rather than the subsequence
/// [`crate::blocks::command_palette::command_palette_matches`] ports: the two
/// upstream components filter differently, and each keeps its own rule. A query
/// that is empty (or only whitespace) matches everything.
pub fn morphing_search_matches(item: &MorphingSearchItem, query: &str) -> bool {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return true;
    }
    let mut hay = item.title.to_lowercase();
    if let Some(description) = &item.description {
        hay.push(' ');
        hay.push_str(&description.to_lowercase());
    }
    for keyword in &item.keywords {
        hay.push(' ');
        hay.push_str(&keyword.to_lowercase());
    }
    hay.contains(&needle)
}

// ---- The trigger -----------------------------------------------------------

/// A view-held open-change callback (erased on build).
type OnOpenChange<State> = Rc<dyn Fn(&mut State, bool)>;

/// What the trigger paints from.
#[derive(Clone, Debug, PartialEq)]
struct TriggerConfig {
    open: bool,
    icon_only: bool,
    placeholder: String,
    shortcut: Option<String>,
}

/// A declarative morphing-search trigger. See [`morphing_search_trigger`].
pub struct MorphingSearchTriggerView<State: 'static> {
    anchor: OverlayAnchor,
    config: TriggerConfig,
    on_open_change: OnOpenChange<State>,
}

/// The closed search affordance: a `w-72` field-shaped button (or a `size-12`
/// icon square) that publishes its own window rect into `anchor` and reports a
/// press as an open request.
///
/// Pair it with [`morphing_search`] over the same anchor; the panel morphs out
/// of this box.
pub fn morphing_search_trigger<State: 'static>(
    anchor: &OverlayAnchor,
) -> MorphingSearchTriggerView<State> {
    MorphingSearchTriggerView {
        anchor: anchor.clone(),
        config: TriggerConfig {
            open: false,
            icon_only: false,
            placeholder: "Search".to_string(),
            shortcut: Some("F".to_string()),
        },
        on_open_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> MorphingSearchTriggerView<State> {
    /// Draw the compact `size-12` icon square instead of the full field
    /// (`iconOnly`).
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.config.icon_only = icon_only;
        self
    }

    /// Set the placeholder the closed trigger shows (`placeholder`).
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.config.placeholder = placeholder.into();
        self
    }

    /// Set the key cap the trigger draws (`shortcut`), or clear it with `None`.
    ///
    /// Drawing it is all this does — the binding itself is the app's, as the
    /// [module docs](self) record.
    pub fn shortcut(mut self, shortcut: Option<impl Into<String>>) -> Self {
        self.config.shortcut = shortcut.map(Into::into);
        self
    }

    /// Hand the trigger the app's open flag, so its chrome fades out as the
    /// panel takes over.
    pub fn open(mut self, open: bool) -> Self {
        self.config.open = open;
        self
    }

    /// Set the open-change callback: a primary press reports `true`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        self.on_open_change = Rc::new(on_open_change);
        self
    }
}

/// The retained widget for a [`MorphingSearchTriggerView`].
pub struct MorphingSearchTriggerWidget {
    anchor: OverlayAnchor,
    config: TriggerConfig,
    placeholder: LabelRun,
    shortcut: Option<LabelRun>,
    /// The trigger's own chrome opacity: `1.0` closed, springing to `0.0` as
    /// the panel takes over.
    chrome: Lane,
    /// The frame the current fade was staged on, for its delay.
    staged: Option<FrameTime>,
    pressed: bool,
    on_open_change: ErasedArgCallback<bool>,
}

impl MorphingSearchTriggerWidget {
    /// The trigger's chrome opacity at the last paint.
    pub fn chrome_opacity(&self) -> f64 {
        self.chrome.value()
    }

    /// The box this trigger occupies, in its own space.
    fn box_size(&self) -> Size {
        if self.config.icon_only {
            Size::new(SEARCH_HEIGHT, SEARCH_HEIGHT)
        } else {
            Size::new(SEARCH_TRIGGER_WIDTH, SEARCH_HEIGHT)
        }
    }
}

impl<State: 'static> View<State> for MorphingSearchTriggerView<State> {
    type Element = MorphingSearchTriggerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> MorphingSearchTriggerWidget {
        MorphingSearchTriggerWidget {
            anchor: self.anchor.clone(),
            config: self.config.clone(),
            placeholder: LabelRun::new(self.config.placeholder.clone()),
            shortcut: self.config.shortcut.clone().map(LabelRun::new),
            chrome: Lane::at_rest(
                Ramp::eased(SEARCH_CHROME_FADE, EASE_OUT),
                if self.config.open { 0.0 } else { 1.0 },
            ),
            staged: None,
            pressed: false,
            on_open_change: erase_callback_arg(&self.on_open_change),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut MorphingSearchTriggerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        element.anchor = self.anchor.clone();
        if element.config != self.config {
            if element.config.icon_only != self.config.icon_only {
                flags |= ChangeFlags::LAYOUT;
            }
            if element.config.open != self.config.open {
                // The fade is delayed, and timed from the frame it next paints.
                element.staged = None;
            }
            element
                .placeholder
                .set_content(self.config.placeholder.clone());
            match (&mut element.shortcut, &self.config.shortcut) {
                (Some(run), Some(text)) => {
                    run.set_content(text.clone());
                }
                (slot @ None, Some(text)) => *slot = Some(LabelRun::new(text.clone())),
                (slot @ Some(_), None) => *slot = None,
                (None, None) => {}
            }
            element.config = self.config.clone();
            flags |= ChangeFlags::PAINT;
        }
        element.on_open_change = erase_callback_arg(&self.on_open_change);
        flags
    }

    fn teardown(&self, _element: &mut MorphingSearchTriggerWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for MorphingSearchTriggerWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let (row_style, small_style) = (row_style(theme), small_style(theme));
        self.placeholder.layout(ctx, &row_style);
        if let Some(shortcut) = &mut self.shortcut {
            shortcut.layout(ctx, &small_style);
        }
        bc.constrain(self.box_size())
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // `PaintCtx::origin` is absolute window space — the read that turns this
        // trigger's own box into the anchor rect the host places against.
        let origin = ctx.origin();
        self.anchor.set(Rect::from_origin_size(origin, ctx.size()));

        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let fill = theme.map_or(crate::BEUI_LIGHT.background, |t| t.scheme().surface);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();

        // The delayed fade: staged on the rebuild that flipped `open`, started
        // once its delay has run out on the paint clock.
        let staged = *self.staged.get_or_insert(now);
        let target = if self.config.open { 0.0 } else { 1.0 };
        if reduce {
            self.chrome.retarget(target);
            self.chrome.snap();
        } else if now.saturating_sub(staged) >= SEARCH_CHROME_DELAY {
            self.chrome
                .retarget_with(Ramp::eased(SEARCH_CHROME_FADE, EASE_OUT), target);
        } else {
            ctx.request_frame();
        }
        if self.chrome.advance(now) {
            ctx.request_frame();
        }
        let opacity = self.chrome.value().clamp(0.0, 1.0);
        if opacity <= 0.0 {
            return;
        }

        let size = ctx.size();
        let layered = opacity < 1.0;
        if layered {
            scene.push_layer(origin, size, opacity as f32);
        }
        scene.fill_rounded_rect(
            origin,
            size,
            SEARCH_RADIUS,
            style::with_alpha(fill, SEARCH_TRIGGER_FILL_ALPHA),
        );
        // `boxShadow: inset 0 0 0 1px …` — an inset hairline, not a border.
        paint_panel_hairline(scene, origin, size, SEARCH_RADIUS, chrome.border);

        let mark_x = if self.config.icon_only {
            size.width / 2.0
        } else {
            SEARCH_PADDING_X + style::ICON_SIZE / 2.0
        };
        draw_search(
            scene,
            origin + Vec2::new(mark_x, size.height / 2.0),
            style::ICON_SIZE,
            chrome.dim_ink,
        );
        if !self.config.icon_only {
            let text = self.placeholder.size();
            self.placeholder.paint(
                origin
                    + Vec2::new(
                        SEARCH_PADDING_X + style::ICON_SIZE + SEARCH_GAP,
                        (size.height - text.height) / 2.0,
                    ),
                chrome.dim_ink,
                scene,
            );
            if let Some(shortcut) = &self.shortcut {
                let cap = key_cap_rect(shortcut.size().width, size, SEARCH_PADDING_X);
                paint_key_cap(scene, origin, cap, shortcut, chrome, fill);
            }
        }
        if layered {
            scene.pop_layer();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) if is_activation_key(key) && ctx.has_focus() => {
                (self.on_open_change)(ctx, true);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => {
                if !presses(p) || !inside(p.position, ctx.size()) {
                    return EventResult::Ignored;
                }
                match p.phase {
                    PointerPhase::Move => {
                        ctx.claim_hover();
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                        EventResult::Ignored
                    }
                    PointerPhase::Down => {
                        self.pressed = true;
                        ctx.capture_pointer();
                        // Focus is what makes Escape and the arrows reachable
                        // once the panel is up.
                        ctx.request_focus();
                        EventResult::Handled
                    }
                    PointerPhase::Up => {
                        if self.pressed {
                            self.pressed = false;
                            (self.on_open_change)(ctx, true);
                        }
                        EventResult::Handled
                    }
                    // A `Cancel` arm never reaches app state.
                    PointerPhase::Cancel => {
                        if !self.pressed {
                            return EventResult::Ignored;
                        }
                        self.pressed = false;
                        EventResult::Handled
                    }
                }
            }
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.config.placeholder.clone();
        ctx.push_node(Role::Button, |node| {
            node.set_label(label.as_str());
            node.add_action(Action::Click);
        });
    }
}

// ---- The component ---------------------------------------------------------

/// The mutable configuration the outer builder writes and the inner panel reads.
type PanelHandle = Rc<RefCell<PanelConfig>>;

/// The close hook, written after the panel view already exists.
type CloseHandle<State> = Rc<RefCell<Option<Rc<dyn Fn(&mut State)>>>>;

/// A view-held query-edit callback (erased on build).
type OnQueryChange<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held selection callback (erased on build).
type OnSelect<State> = Rc<dyn Fn(&mut State, usize)>;

/// What the panel needs from its component's builders.
#[derive(Clone, Debug)]
struct PanelConfig {
    open: bool,
    placeholder: String,
    empty: String,
    anchor: OverlayAnchor,
}

/// Equality over what the panel renders from; the shared [`OverlayAnchor`] cell
/// carries no identity comparison and is re-seated unconditionally instead.
impl PartialEq for PanelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.open == other.open
            && self.placeholder == other.placeholder
            && self.empty == other.empty
    }
}

/// A declarative beUI morphing search. See [`morphing_search`].
pub struct MorphingSearchView<State: 'static> {
    inner: AnchoredOverlayView<State>,
    config: PanelHandle,
    close: CloseHandle<State>,
}

/// Build the search panel over `items`, filtered by `query`, to be mounted as
/// the top child of a full-area [`frust::Stack`] and anchored to a
/// [`morphing_search_trigger`].
///
/// `on_query_change(state, text)` reports each edit, and `on_select(state,
/// index)` reports an activation with the item's index into the **unfiltered**
/// `items` vector. A selection also closes the panel, upstream's own
/// `selectItem` → `closeSearch`.
pub fn morphing_search<State: 'static, F, G>(
    items: Vec<MorphingSearchItem>,
    query: impl Into<String>,
    on_query_change: F,
    on_select: G,
) -> MorphingSearchView<State>
where
    F: Fn(&mut State, String) + 'static,
    G: Fn(&mut State, usize) + 'static,
{
    let config: PanelHandle = Rc::new(RefCell::new(PanelConfig {
        open: true,
        placeholder: "Search".to_string(),
        empty: "No results found.".to_string(),
        anchor: OverlayAnchor::new(),
    }));
    let close: CloseHandle<State> = Rc::new(RefCell::new(None));
    let panel = SearchPanelView {
        items,
        query: query.into(),
        config: config.clone(),
        close: close.clone(),
        on_query_change: Rc::new(on_query_change),
        on_select: Rc::new(on_select),
    };
    // Anchored to the trigger's own top-left corner: `bottom`/`start` at minus
    // the trigger's height puts the panel's top edge on the anchor's, which is
    // what upstream's `top: anchorRect.top, left: anchorRect.left` does.
    let placement = OverlayPlacement::on(OverlaySide::Bottom)
        .align(OverlayAlign::Start)
        .offset(-SEARCH_HEIGHT)
        .flip(false);
    MorphingSearchView {
        // The host's own scale/fade is switched off — this panel's morph *is*
        // its entrance — and its exit is held open for the fold-back, the same
        // arrangement `center_morph_modal` makes with the modal host.
        inner: anchored(panel)
            .placement(placement)
            .enter(Ramp::eased(Duration::ZERO, EASE_OUT))
            .exit(Ramp::spring(SEARCH_SHELL)),
        config,
        close,
    }
}

impl<State: 'static> MorphingSearchView<State> {
    /// Anchor the panel to the trigger's published rect.
    pub fn anchor(mut self, anchor: &OverlayAnchor) -> Self {
        self.config.borrow_mut().anchor = anchor.clone();
        self.inner = self.inner.anchor(anchor);
        self
    }

    /// Hand the panel the app's open flag. The default is `true`.
    pub fn open(mut self, open: bool) -> Self {
        self.config.borrow_mut().open = open;
        self.inner = self.inner.open(open);
        self
    }

    /// Set the field's placeholder.
    pub fn placeholder(self, placeholder: impl Into<String>) -> Self {
        self.config.borrow_mut().placeholder = placeholder.into();
        self
    }

    /// Set the text shown when nothing matches the query.
    pub fn empty(self, empty: impl Into<String>) -> Self {
        self.config.borrow_mut().empty = empty.into();
        self
    }

    /// Set the open-change callback: a press outside, Escape, or a selection all
    /// report `false`.
    pub fn on_open_change<F: Fn(&mut State, bool) + 'static>(mut self, on_open_change: F) -> Self {
        let shared = Rc::new(on_open_change);
        let host = shared.clone();
        self.inner = self.inner.on_dismiss(move |state| host(state, false));
        *self.close.borrow_mut() = Some(Rc::new(move |state: &mut State| shared(state, false)));
        self
    }

    /// Set the exit-finished callback — the host has settled closed.
    pub fn on_exited<F: Fn(&mut State) + 'static>(mut self, on_exited: F) -> Self {
        self.inner = self.inner.on_exited(on_exited);
        self
    }
}

impl<State: 'static> View<State> for MorphingSearchView<State> {
    type Element = AnchoredOverlayWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchoredOverlayWidget {
        View::build(&self.inner, ctx)
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchoredOverlayWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        View::rebuild(&self.inner, &prev.inner, element, ctx)
    }

    fn teardown(&self, element: &mut AnchoredOverlayWidget, ctx: &mut BuildCtx<'_>) {
        View::teardown(&self.inner, element, ctx);
    }
}

// ---- The panel -------------------------------------------------------------

/// The search panel: the morphing shell, the query row and the results.
struct SearchPanelView<State: 'static> {
    items: Vec<MorphingSearchItem>,
    query: String,
    config: PanelHandle,
    close: CloseHandle<State>,
    on_query_change: OnQueryChange<State>,
    on_select: OnSelect<State>,
}

/// One laid-out result row.
struct Row {
    /// The item's index in the unfiltered list, or `None` for the empty state.
    item: Option<usize>,
    title: LabelRun,
    description: Option<LabelRun>,
    rect: Rect,
}

/// The retained widget for a morphing-search panel.
pub struct MorphingSearchWidget {
    field: ChildPod,
    items: Vec<MorphingSearchItem>,
    query: String,
    rows: Vec<Row>,
    config: PanelConfig,
    kbd: LabelRun,
    /// The morphing shell's width and height — see the [module docs](self) on
    /// why these are lanes rather than one progress ramp.
    shell_w: Lane,
    shell_h: Lane,
    /// The content's own unfold, tweened rather than sprung.
    clip: Presence,
    /// The `reduce_motion` value the drivers were last built for.
    reduced: Option<bool>,
    /// The highlighted row.
    active: Option<usize>,
    /// The row a primary `Down` armed.
    armed: Option<usize>,
    /// The active pill's travel and opacity.
    pill: Lane,
    pill_from: Rect,
    pill_to: Rect,
    pill_alpha: Lane,
    /// The trigger box the shell morphs out of, in this widget's own space.
    source: Rect,
    /// Whether the shell lanes still have to be seated on that source box.
    ///
    /// The lanes are built before the first layout has measured anything, and
    /// an open episode has to start from the *trigger's* box rather than from
    /// wherever the last one left them — so the seating waits for the paint
    /// that follows a layout.
    seed_shell: bool,
    on_query_change: ErasedArgCallback<String>,
    on_select: ErasedArgCallback<usize>,
    on_close: Option<ErasedCallback>,
}

impl MorphingSearchWidget {
    /// The highlighted row.
    pub fn active_row(&self) -> Option<usize> {
        self.active
    }

    /// The shell's box right now, in this widget's own space — the trigger's
    /// box while closed, the panel's once the morph has settled.
    pub fn shell_rect(&self) -> Rect {
        Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(self.shell_w.value(), self.shell_h.value()),
        )
    }

    /// The content unfold's phase.
    pub fn clip_phase(&self) -> PresencePhase {
        self.clip.phase()
    }

    /// The rows the current `items`/`query` pair renders to.
    fn build_rows(items: &[MorphingSearchItem], query: &str, empty: &str) -> Vec<Row> {
        let mut rows: Vec<Row> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| morphing_search_matches(item, query))
            .map(|(index, item)| Row {
                item: Some(index),
                title: LabelRun::new(item.title.clone()),
                description: item.description.clone().map(LabelRun::new),
                rect: Rect::ZERO,
            })
            .collect();
        if rows.is_empty() {
            rows.push(Row {
                item: None,
                title: LabelRun::new(empty.to_string()),
                description: None,
                rect: Rect::ZERO,
            });
        }
        rows
    }

    /// The first selectable row, if the current rows have one.
    fn first_item(&self) -> Option<usize> {
        self.rows.iter().position(|row| row.item.is_some())
    }

    /// Adopt a freshly-built row set, returning the flags it costs.
    fn adopt_rows(&mut self, rows: Vec<Row>) -> ChangeFlags {
        self.rows = rows;
        self.armed = None;
        let first = self.first_item();
        self.active = first;
        self.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 1.0);
        self.pill_from = Rect::ZERO;
        self.pill_to = Rect::ZERO;
        self.pill_alpha = Lane::at_rest(
            Ramp::spring(SPRING_LAYOUT),
            if first.is_some() { 1.0 } else { 0.0 },
        );
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }

    /// Move the pill onto `next`, springing from wherever it is now.
    fn set_active(&mut self, next: Option<usize>) -> bool {
        if self.active == next {
            return false;
        }
        let current = self.pill_rect();
        self.active = next;
        if let Some(row) = next.and_then(|i| self.rows.get(i)) {
            self.pill_from = if self.pill_alpha.target() == 0.0 && current.is_zero_area() {
                row.rect
            } else {
                current
            };
            self.pill_to = row.rect;
            self.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 0.0);
            self.pill.retarget(1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(SPRING_LAYOUT), 1.0);
        } else {
            self.pill_from = current;
            self.pill_to = current;
            self.pill = Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 1.0);
            self.pill_alpha
                .retarget_with(Ramp::spring(SPRING_LAYOUT), 0.0);
        }
        true
    }

    /// The pill's rect right now.
    fn pill_rect(&self) -> Rect {
        if self.pill_from.is_zero_area() && self.pill_to.is_zero_area() {
            return self
                .active
                .and_then(|i| self.rows.get(i))
                .map_or(Rect::ZERO, |row| row.rect);
        }
        let t = self.pill.value().clamp(0.0, 1.0);
        Rect::new(
            lerp(self.pill_from.x0, self.pill_to.x0, t),
            lerp(self.pill_from.y0, self.pill_to.y0, t),
            lerp(self.pill_from.x1, self.pill_to.x1, t),
            lerp(self.pill_from.y1, self.pill_to.y1, t),
        )
    }

    /// The next selectable row in `step`'s direction, clamped at both ends —
    /// upstream's `useRowCursor.moveActive`, which never wraps.
    fn step_active(&self, step: isize) -> Option<usize> {
        let items: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.item.is_some())
            .map(|(i, _)| i)
            .collect();
        if items.is_empty() {
            return None;
        }
        let at = self
            .active
            .and_then(|row| items.iter().position(|i| *i == row))
            .unwrap_or(0) as isize;
        let last = items.len() as isize - 1;
        Some(items[at.saturating_add(step).clamp(0, last) as usize])
    }

    /// The selectable row `position` (in this widget's own space) lands on.
    fn row_at(&self, position: Point) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.item.is_some() && row.rect.contains(position))
    }

    /// Report `row`'s activation, then close — upstream's `selectItem`.
    fn select(&mut self, ctx: &mut EventCtx, row: usize) {
        let Some(Some(index)) = self.rows.get(row).map(|row| row.item) else {
            return;
        };
        (self.on_select)(ctx, index);
        if let Some(close) = self.on_close.as_mut() {
            close(ctx);
        }
    }

    /// Rebuild the unfold driver when `reduce_motion` flips, preserving what the
    /// old one was doing.
    fn sync_motion(&mut self, reduce: bool) {
        if self.reduced == Some(reduce) {
            return;
        }
        let was_exiting = self.clip.phase() == PresencePhase::Exiting;
        self.reduced = Some(reduce);
        let base = Presence::symmetric(Ramp::eased(SEARCH_CLIP, EASE_OUT));
        let mut next = if reduce { base.collapsed() } else { base };
        if was_exiting && !self.config.open {
            next.set_open(true);
        }
        next.set_open(self.config.open);
        self.clip = next;
    }
}

impl<State: 'static> SearchPanelView<State> {
    /// The wrapped baseline field, with its own chrome suppressed.
    fn field(&self) -> AnyView<State> {
        let on_change = self.on_query_change.clone();
        any(
            // Not themed: the baseline `text_input` has no themed-family opt-in.
            text_input(self.query.clone(), move |state: &mut State, text| {
                on_change(state, text)
            })
            .placeholder(self.config.borrow().placeholder.clone())
            .padding(0.0, 0.0)
            .border_width(0.0)
            .corner_radius(0.0)
            .focus_ring_width(0.0),
        )
    }
}

impl<State: 'static> View<State> for SearchPanelView<State> {
    type Element = MorphingSearchWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> MorphingSearchWidget {
        let config = self.config.borrow().clone();
        let rows = MorphingSearchWidget::build_rows(&self.items, &self.query, &config.empty);
        let first = rows.iter().position(|row| row.item.is_some());
        let mut clip = Presence::symmetric(Ramp::eased(SEARCH_CLIP, EASE_OUT));
        clip.set_open(config.open);
        MorphingSearchWidget {
            field: build_child(&self.field(), ctx),
            items: self.items.clone(),
            query: self.query.clone(),
            rows,
            config,
            kbd: LabelRun::new(SEARCH_KBD_LABEL),
            shell_w: Lane::at_rest(Ramp::spring(SEARCH_SHELL), 0.0),
            shell_h: Lane::at_rest(Ramp::spring(SEARCH_SHELL), 0.0),
            clip,
            reduced: None,
            active: first,
            armed: None,
            pill: Lane::at_rest(Ramp::spring(SPRING_LAYOUT), 1.0),
            pill_from: Rect::ZERO,
            pill_to: Rect::ZERO,
            pill_alpha: Lane::at_rest(
                Ramp::spring(SPRING_LAYOUT),
                if first.is_some() { 1.0 } else { 0.0 },
            ),
            source: Rect::ZERO,
            seed_shell: true,
            on_query_change: erase_callback_arg(&self.on_query_change),
            on_select: erase_callback_arg(&self.on_select),
            on_close: self.close.borrow().as_ref().map(erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut MorphingSearchWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(&prev.field(), &self.field(), &mut element.field, ctx);
        let config = self.config.borrow().clone();
        element.config.anchor = config.anchor.clone();
        if element.config != config {
            let opened = config.open && !element.config.open;
            element.config.open = config.open;
            element.config.placeholder = config.placeholder.clone();
            element.config.empty = config.empty.clone();
            element.clip.set_open(config.open);
            if opened {
                element.armed = None;
                element.active = element.first_item();
                element.seed_shell = true;
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.items != self.items || element.query != self.query {
            element.items = self.items.clone();
            element.query = self.query.clone();
            let rows =
                MorphingSearchWidget::build_rows(&element.items, &element.query, &config.empty);
            flags |= element.adopt_rows(rows);
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_query_change = erase_callback_arg(&self.on_query_change);
        element.on_select = erase_callback_arg(&self.on_select);
        element.on_close = self.close.borrow().as_ref().map(erase_callback);
        flags
    }

    fn teardown(&self, element: &mut MorphingSearchWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.field(), &mut element.field, ctx);
    }
}

// ---- Shared chrome ---------------------------------------------------------

/// A row title's / placeholder's style (`text-sm font-medium`), in the
/// theme's `label_large` family.
fn row_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle {
        weight: FontWeight::MEDIUM,
        ..TextStyle::new(style::TEXT_SM as f32, crate::text::SHAPING_INK)
    };
    themed_style(style, ThemeTextType::LabelLarge, theme)
}

/// A description's / key cap's style (`text-xs`), in the theme's
/// `label_small` family.
fn small_style(theme: Option<&Theme>) -> TextStyle {
    let style = TextStyle::new(style::TEXT_XS as f32, crate::text::SHAPING_INK);
    themed_style(style, ThemeTextType::LabelSmall, theme)
}

/// The key cap's box at the trailing edge of a `size`-shaped row.
fn key_cap_rect(text_width: f64, size: Size, padding_x: f64) -> Rect {
    let width = text_width + SEARCH_KBD_PADDING_X * 2.0;
    Rect::new(
        size.width - padding_x - width,
        (size.height - SEARCH_KBD_HEIGHT) / 2.0,
        size.width - padding_x,
        (size.height + SEARCH_KBD_HEIGHT) / 2.0,
    )
}

/// Paint a `rounded-md border` key cap holding `label`.
fn paint_key_cap(
    scene: &mut dyn PaintScene,
    origin: Point,
    cap: Rect,
    label: &LabelRun,
    chrome: PanelChrome,
    fill: Color,
) {
    let at = origin + cap.origin().to_vec2();
    scene.fill_rounded_rect(at, cap.size(), style::RADIUS_MD, fill);
    paint_panel_hairline(scene, at, cap.size(), style::RADIUS_MD, chrome.border);
    let text = label.size();
    label.paint(
        at + Vec2::new(SEARCH_KBD_PADDING_X, (cap.height() - text.height) / 2.0),
        chrome.dim_ink,
        scene,
    );
}

impl Widget for MorphingSearchWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        let row_style = row_style(theme);
        let small_style = small_style(theme);

        // The panel is as wide as the trigger at the least, capped at upstream's
        // `448` against the area it is placed inside.
        let anchor = self.config.anchor.rect();
        let area = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            SEARCH_MAX_WIDTH
        };
        let width = anchor
            .width()
            .max(SEARCH_MAX_WIDTH.min(area - crate::overlay::VIEWPORT_PADDING * 2.0))
            .max(SEARCH_TRIGGER_WIDTH.min(area));
        self.source = Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(
                if anchor.width() > 0.0 {
                    anchor.width().min(width)
                } else {
                    SEARCH_TRIGGER_WIDTH.min(width)
                },
                SEARCH_HEIGHT,
            ),
        );

        // The query row: the mark leads, the key cap trails, the field takes the
        // rest.
        let kbd_width = self.kbd.layout(ctx, &small_style).width + SEARCH_KBD_PADDING_X * 2.0;
        let lead = SEARCH_PADDING_X + style::ICON_SIZE + SEARCH_GAP;
        let field_width = (width - lead - kbd_width - SEARCH_GAP - SEARCH_PADDING_X).max(0.0);
        self.field.layout_child(
            ctx,
            &BoxConstraints::tight(Size::new(field_width, SEARCH_HEIGHT)),
        );
        self.field.set_origin(Point::new(lead, 0.0));

        // The results, stacked inside the list's padding and capped.
        let mut y = SEARCH_HEIGHT + style::BORDER_WIDTH + SEARCH_LIST_PADDING;
        let list_top = y;
        for row in &mut self.rows {
            row.title.layout(ctx, &row_style);
            if let Some(description) = &mut row.description {
                description.layout(ctx, &small_style);
            }
            let height = row.height();
            row.rect = Rect::new(
                SEARCH_LIST_PADDING,
                y,
                width - SEARCH_LIST_PADDING,
                y + height,
            );
            y += height;
        }
        let list_height = (y - list_top + SEARCH_LIST_PADDING).min(SEARCH_RESULTS_MAX_HEIGHT);
        if self.pill_from.is_zero_area() && self.pill_to.is_zero_area() {
            let seat = self.pill_rect();
            self.pill_from = seat;
            self.pill_to = seat;
        }
        bc.constrain(Size::new(width, list_top + list_height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let cap_fill = theme.map_or(crate::BEUI_LIGHT.background, |t| t.scheme().surface);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        self.sync_motion(reduce);
        let now = ctx.frame_time();
        let origin = ctx.origin();

        let progress = self.clip.advance(now);
        if self.clip.is_animating() {
            ctx.request_frame();
        }
        let panel = Rect::from_origin_size(Point::ORIGIN, ctx.size());

        // A fresh open episode starts from the trigger's own box.
        if self.seed_shell {
            self.seed_shell = false;
            self.shell_w.retarget(self.source.width());
            self.shell_h.retarget(self.source.height());
            self.shell_w.snap();
            self.shell_h.snap();
        }

        // The shell's two lanes, re-aimed at whichever box is current. A query
        // that resizes the panel mid-morph retargets them from what is on
        // screen rather than restarting the run.
        let target = if self.config.open {
            panel.size()
        } else {
            self.source.size()
        };
        self.shell_w.retarget(target.width);
        self.shell_h.retarget(target.height);
        if reduce {
            self.shell_w.snap();
            self.shell_h.snap();
        }
        let moving = self.shell_w.advance(now) | self.shell_h.advance(now);
        if moving {
            ctx.request_frame();
        }
        if !self.clip.is_visible() && !moving {
            return;
        }

        let shell = Rect::from_origin_size(
            Point::ORIGIN,
            Size::new(self.shell_w.value().max(0.0), self.shell_h.value().max(0.0)),
        );
        paint_panel(scene, origin, shell, SEARCH_RADIUS, chrome);

        // The content unfolds on its own tween, from the trigger's own box out
        // to the panel — `collapsedContentClip` → `expandedContentClip`.
        let (clip, radius) = morph_rect(self.source, SEARCH_RADIUS, panel, SEARCH_RADIUS, progress);
        let clip = Rect::new(
            clip.x0,
            clip.y0,
            clip.x1.min(shell.x1),
            clip.y1.min(shell.y1),
        );
        if clip.width() <= 0.0 || clip.height() <= 0.0 {
            return;
        }
        scene.push_clip_rounded(origin + clip.origin().to_vec2(), clip.size(), radius);
        let faded = progress < 1.0;
        if faded {
            scene.push_layer(
                origin + clip.origin().to_vec2(),
                clip.size(),
                progress.clamp(0.0, 1.0) as f32,
            );
        }

        // The query row.
        draw_search(
            scene,
            origin
                + Vec2::new(
                    SEARCH_PADDING_X + style::ICON_SIZE / 2.0,
                    SEARCH_HEIGHT / 2.0,
                ),
            style::ICON_SIZE,
            chrome.dim_ink,
        );
        self.field.paint_child(ctx, scene);
        let cap = key_cap_rect(
            self.kbd.size().width,
            Size::new(ctx.size().width, SEARCH_HEIGHT),
            SEARCH_PADDING_X,
        );
        paint_key_cap(scene, origin, cap, &self.kbd, chrome, cap_fill);
        scene.fill_rect(
            origin + Vec2::new(0.0, SEARCH_HEIGHT),
            Size::new(ctx.size().width, style::BORDER_WIDTH),
            chrome.border,
        );

        self.paint_rows(ctx, scene, chrome, now);

        if faded {
            scene.pop_layer();
        }
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if event.is_broadcast() {
            self.field.event_child(ctx, event);
            return EventResult::Ignored;
        }
        if !self.config.open {
            return EventResult::Ignored;
        }
        if let InputEvent::Key(key) = event {
            return self.handle_key(ctx, event, key);
        }
        // The IME session and the clipboard verbs an `EditCommand` carries both
        // belong to the field outright — `Key` is handled above, since the
        // widget's own navigation keys intercept before falling through to
        // `handle_key`'s own field forward. Branch on the shared predicate
        // rather than enumerating `Ime`/`EditCommand` separately.
        if event.is_focus_routed() {
            return route_event_single(&mut self.field, ctx, event);
        }
        match event {
            InputEvent::Pointer(p) => self.handle_pointer(ctx, event, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if !self.config.open {
            return;
        }
        self.field.semantics_child(ctx);
        let active = self.active;
        ctx.push_container(
            Role::ListBox,
            |_| {},
            |ctx| {
                for (index, row) in self.rows.iter().enumerate() {
                    if row.item.is_none() {
                        continue;
                    }
                    ctx.push_node(Role::ListBoxOption, |node| {
                        node.set_label(row.title.content());
                        node.set_selected(active == Some(index));
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }

    visit_children!(field);
}

impl MorphingSearchWidget {
    /// The `Widget::event` key arm — the same three intercepted keys the
    /// palette documents, with Escape left for the host.
    fn handle_key(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        key: &KeyEvent,
    ) -> EventResult {
        match &key.key {
            Key::Named(NamedKey::Escape) => EventResult::Ignored,
            Key::Named(NamedKey::ArrowDown) => {
                if self.set_active(self.step_active(1)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.set_active(self.step_active(-1)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            Key::Named(NamedKey::Enter) => {
                let Some(row) = self.active else {
                    return EventResult::Ignored;
                };
                self.select(ctx, row);
                EventResult::Handled
            }
            _ => route_event_single(&mut self.field, ctx, event),
        }
    }

    /// The `Widget::event` pointer arm: the query row is the field's, the
    /// results are this widget's.
    fn handle_pointer(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
    ) -> EventResult {
        if !presses(p) && p.phase != PointerPhase::Move {
            return EventResult::Ignored;
        }
        if p.position.y < SEARCH_HEIGHT || self.field.is_active() {
            return route_event_single(&mut self.field, ctx, event);
        }
        if !inside(p.position, ctx.size()) {
            return EventResult::Ignored;
        }
        let row = self.row_at(p.position);
        match p.phase {
            PointerPhase::Move => {
                if row.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if row.is_some() && self.set_active(row) {
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                let Some(row) = row else {
                    // A press on the panel's own chrome still claims focus, so
                    // the arrows and Escape keep working.
                    ctx.request_focus();
                    return EventResult::Handled;
                };
                ctx.request_focus();
                ctx.capture_pointer();
                self.armed = Some(row);
                if self.set_active(Some(row)) {
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if row == Some(armed) {
                    self.select(ctx, armed);
                }
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                if self.armed.take().is_none() {
                    return EventResult::Ignored;
                }
                EventResult::Handled
            }
        }
    }

    /// Paint the active pill and every result row.
    fn paint_rows(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        chrome: PanelChrome,
        now: FrameTime,
    ) {
        let origin = ctx.origin();
        let travelled = self.pill.advance(now);
        let faded = self.pill_alpha.advance(now);
        if travelled || faded {
            ctx.request_frame();
        }
        let alpha = self.pill_alpha.value().clamp(0.0, 1.0) as f32;
        if alpha > 0.0 {
            let pill = self.pill_rect();
            if pill.width() > 0.0 && pill.height() > 0.0 {
                scene.fill_rounded_rect(
                    origin + pill.origin().to_vec2(),
                    pill.size(),
                    SEARCH_ROW_RADIUS,
                    style::with_alpha(chrome.ink, SEARCH_PILL_ALPHA * alpha),
                );
            }
        }
        for row in &self.rows {
            row.paint(origin, chrome, scene);
        }
    }
}

impl Row {
    /// This row's own height, in logical px.
    fn height(&self) -> f64 {
        match (self.item.is_some(), self.description.is_some()) {
            (false, _) => SEARCH_EMPTY_HEIGHT,
            (true, true) => SEARCH_ROW_HEIGHT + SEARCH_ROW_DESCRIPTION_HEIGHT,
            (true, false) => SEARCH_ROW_HEIGHT,
        }
    }

    /// Paint this row's title (and description), or the centred empty line.
    fn paint(&self, origin: Point, chrome: PanelChrome, scene: &mut dyn PaintScene) {
        let title = self.title.size();
        if self.item.is_none() {
            // `text-center`, resolved here since the catalog shapes its own runs.
            let x = self.rect.x0 + (self.rect.width() - title.width) / 2.0;
            let y = self.rect.y0 + (self.rect.height() - title.height) / 2.0;
            self.title
                .paint(origin + Vec2::new(x, y), chrome.dim_ink, scene);
            return;
        }
        let x = self.rect.x0 + SEARCH_ROW_PADDING_X;
        match &self.description {
            None => {
                let y = self.rect.y0 + (self.rect.height() - title.height) / 2.0;
                self.title
                    .paint(origin + Vec2::new(x, y), chrome.ink, scene);
            }
            Some(description) => {
                let y = self.rect.y0 + SEARCH_ROW_PADDING_Y;
                self.title
                    .paint(origin + Vec2::new(x, y), chrome.ink, scene);
                description.paint(
                    origin + Vec2::new(x, y + title.height),
                    chrome.dim_ink,
                    scene,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, escape, ft_ms, light, pointer, reduced};
    use frust::authoring::text::TextContext;
    use frust::authoring::{EditCommand, Key, KeyEvent, Modifiers, NamedKey};
    use frust_core::RenderRoot;
    use std::any::Any;

    /// A window wide enough for the panel's own `448` cap to bind.
    const WINDOW: Size = Size::new(800.0, 600.0);

    /// Where the trigger sits inside the window, so a published anchor proves it
    /// is absolute rather than region-local.
    const INSET: f64 = 24.0;

    /// Long enough for the shell springs and the content tween to settle.
    const SETTLE_MS: f64 = 3_000.0;

    #[derive(Default)]
    struct App {
        open: bool,
        opens: Vec<bool>,
        query: String,
        selected: Vec<usize>,
        icon_only: bool,
    }

    fn items() -> Vec<MorphingSearchItem> {
        vec![
            morphing_search_item("Overview")
                .description("Dashboard and metrics")
                .keywords(["home"]),
            morphing_search_item("Billing").description("Invoices and plans"),
            morphing_search_item("Members"),
            morphing_search_item("Settings").keywords(["preferences"]),
        ]
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        anchor: OverlayAnchor,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::themed(light())
        }

        fn themed(theme: Theme) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App::default(),
                tcx: TextContext::new(),
                anchor: OverlayAnchor::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(theme));
            h.step(0.0);
            h
        }

        /// Advance the clock by `ms` and run a whole frame.
        fn step(&mut self, ms: f64) {
            self.clock += ms;
            let anchor = self.anchor.clone();
            let mut logic = move |s: &mut App| {
                frust::stack()
                    .child(frust::Padding(
                        frust::EdgeInsets::all(INSET),
                        morphing_search_trigger(&anchor)
                            .icon_only(s.icon_only)
                            .open(s.open)
                            .on_open_change(|s: &mut App, open| {
                                s.open = open;
                                s.opens.push(open);
                            }),
                    ))
                    .child(
                        morphing_search(
                            items(),
                            s.query.clone(),
                            |s: &mut App, text| s.query = text,
                            |s: &mut App, index| s.selected.push(index),
                        )
                        .anchor(&anchor)
                        .open(s.open)
                        .on_open_change(|s: &mut App, open| {
                            s.open = open;
                            s.opens.push(open);
                        }),
                    )
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let now = self.clock;
            self.root.paint(&mut Recorder::default(), ft_ms(now));
        }

        fn read_after(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut rec = Recorder::default();
            self.root.paint(&mut rec, ft_ms(self.clock));
            rec
        }

        fn read(&mut self) -> Recorder {
            self.read_after(SETTLE_MS)
        }

        fn settle(&mut self) {
            self.step(SETTLE_MS);
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        /// Press the trigger and settle the morph.
        fn open(&mut self) {
            let at = Point::new(INSET + 40.0, INSET + SEARCH_HEIGHT / 2.0);
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.step(0.0);
            self.step(16.0);
            self.settle();
        }

        /// The panel's shell box in window space.
        ///
        /// The trigger and the shell share both their radius and their origin,
        /// so they are told apart by paint order: the panel is the `Stack`'s
        /// last child, and so the last rounded rect at the shared radius.
        fn shell(&mut self, rec: &Recorder) -> Rect {
            let _ = self;
            let (origin, size, _, _) = *rec
                .rrects
                .iter()
                .rfind(|(_, _, r, _)| *r == SEARCH_RADIUS)
                .expect("the shell");
            Rect::from_origin_size(origin, size)
        }

        /// The settled shell.
        fn settled_shell(&mut self) -> Rect {
            let rec = self.read();
            self.shell(&rec)
        }

        /// Press inside the panel's query row, which is what puts the panel on
        /// the focus chain — the trigger's own press left the focus on the
        /// trigger, and there is no focus-on-appear hook.
        fn focus_panel(&mut self) {
            let shell = self.settled_shell();
            let at = shell.origin() + Vec2::new(200.0, SEARCH_HEIGHT / 2.0);
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.settle();
        }

        fn key(&mut self, key: NamedKey) {
            self.event(InputEvent::Key(KeyEvent {
                key: Key::Named(key),
                modifiers: Modifiers::default(),
                repeat: false,
            }));
            self.settle();
        }

        /// The active pill's box, once its travel has settled.
        fn pill(&mut self) -> Rect {
            let rec = self.read();
            let (origin, size, _, _) = *rec
                .rrects
                .iter()
                .find(|(_, _, r, _)| *r == SEARCH_ROW_RADIUS)
                .expect("the active pill");
            Rect::from_origin_size(origin, size)
        }
    }

    /// The panel height a list of the given row heights lays out to.
    fn expected_height(rows: f64) -> f64 {
        SEARCH_HEIGHT + style::BORDER_WIDTH + SEARCH_LIST_PADDING * 2.0 + rows
    }

    // ---- The filter -------------------------------------------------------

    #[test]
    fn the_filter_is_a_case_folded_substring_over_title_description_and_keywords() {
        let item = morphing_search_item("Overview")
            .description("Dashboard and metrics")
            .keywords(["home"]);
        assert!(morphing_search_matches(&item, ""));
        assert!(morphing_search_matches(&item, "   "), "whitespace only");
        assert!(morphing_search_matches(&item, "VIEW"), "case-folded");
        assert!(morphing_search_matches(&item, "metrics"), "the description");
        assert!(morphing_search_matches(&item, "home"), "a keyword");
        assert!(
            !morphing_search_matches(&item, "ovw"),
            "a substring, not the palette's subsequence"
        );
    }

    // ---- The trigger ------------------------------------------------------

    #[test]
    fn the_trigger_publishes_its_own_window_box_and_opens_on_a_press() {
        let mut h = Harness::new();
        assert_eq!(
            h.anchor.rect(),
            Rect::from_origin_size(
                Point::new(INSET, INSET),
                Size::new(SEARCH_TRIGGER_WIDTH, SEARCH_HEIGHT)
            ),
            "published in absolute window space"
        );
        let at = Point::new(INSET + 40.0, INSET + SEARCH_HEIGHT / 2.0);
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        assert!(
            h.state.opens.is_empty(),
            "an open fires on up-inside, not down"
        );
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        assert_eq!(h.state.opens, vec![true]);
    }

    #[test]
    fn the_icon_only_trigger_is_square() {
        let mut h = Harness::new();
        h.state.icon_only = true;
        h.settle();
        assert_eq!(
            h.anchor.rect().size(),
            Size::new(SEARCH_HEIGHT, SEARCH_HEIGHT)
        );
    }

    #[test]
    fn the_triggers_chrome_fades_out_once_the_panel_has_taken_over() {
        let mut h = Harness::new();
        let closed = h.read_after(0.0);
        let before = closed
            .rrects
            .iter()
            .filter(|(_, _, r, _)| *r == SEARCH_RADIUS)
            .count();
        assert_eq!(before, 1, "just the trigger while closed");

        h.open();
        let open = h.read();
        let after: Vec<_> = open
            .rrects
            .iter()
            .filter(|(_, _, r, _)| *r == SEARCH_RADIUS)
            .collect();
        assert_eq!(
            after.len(),
            1,
            "the faded-out trigger paints nothing; only the shell is left"
        );
    }

    // ---- Placement and the morph ------------------------------------------

    #[test]
    fn the_panel_opens_over_the_trigger_and_settles_at_the_capped_width() {
        let mut h = Harness::new();
        h.open();
        let shell = h.settled_shell();
        assert_eq!(
            shell.origin(),
            Point::new(INSET, INSET),
            "the panel's top-left corner is the trigger's"
        );
        assert_eq!(shell.width(), SEARCH_MAX_WIDTH);
        // Two described rows and two bare ones.
        assert_eq!(
            shell.height(),
            expected_height(
                2.0 * (SEARCH_ROW_HEIGHT + SEARCH_ROW_DESCRIPTION_HEIGHT) + 2.0 * SEARCH_ROW_HEIGHT
            )
        );
    }

    #[test]
    fn the_shell_grows_out_of_the_triggers_own_box() {
        let mut h = Harness::new();
        let at = Point::new(INSET + 40.0, INSET + SEARCH_HEIGHT / 2.0);
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        h.step(0.0);
        // The first painted frame of the run is the trigger's box exactly — the
        // lanes are seeded there, not at zero and not at the panel's size.
        let first = h.read_after(0.0);
        let shell = h.shell(&first);
        assert_eq!(shell.size(), Size::new(SEARCH_TRIGGER_WIDTH, SEARCH_HEIGHT));

        let mid = h.read_after(60.0);
        let shell = h.shell(&mid);
        assert!(
            shell.width() > SEARCH_TRIGGER_WIDTH && shell.width() < SEARCH_MAX_WIDTH,
            "mid-morph: {shell:?}"
        );
        assert!(shell.height() > SEARCH_HEIGHT);
    }

    #[test]
    fn a_query_that_resizes_the_panel_mid_morph_retargets_the_shell_continuously() {
        let mut h = Harness::new();
        let at = Point::new(INSET + 40.0, INSET + SEARCH_HEIGHT / 2.0);
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        h.step(0.0);
        let mid = h.read_after(80.0);
        let flying = h.shell(&mid);
        assert!(
            flying.height() > SEARCH_HEIGHT && flying.height() < expected_height(192.0),
            "caught mid-flight: {flying:?}"
        );

        // Narrowing to one row moves the target under the running lane.
        h.state.query = "bill".to_string();
        h.step(1.0);
        let after = h.read_after(0.0);
        let caught = h.shell(&after);
        assert!(
            (caught.height() - flying.height()).abs() < 1.0,
            "the lane continued from what was on screen ({flying:?} -> {caught:?}), it did not jump"
        );

        // ...and it settles on the *new* target rather than the old one.
        let settled = h.read_after(SETTLE_MS);
        let settled = h.shell(&settled);
        assert_eq!(
            settled.height(),
            expected_height(SEARCH_ROW_HEIGHT + SEARCH_ROW_DESCRIPTION_HEIGHT),
            "one described row"
        );
    }

    #[test]
    fn reduce_motion_lands_the_shell_on_the_panel_at_once() {
        let mut h = Harness::themed(reduced());
        let at = Point::new(INSET + 40.0, INSET + SEARCH_HEIGHT / 2.0);
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        h.step(0.0);
        let first = h.read_after(0.0);
        let shell = h.shell(&first);
        assert_eq!(
            shell.width(),
            SEARCH_MAX_WIDTH,
            "no morph at all: {shell:?}"
        );
    }

    // ---- Results ----------------------------------------------------------

    #[test]
    fn a_narrowing_query_drops_the_rows_it_excludes() {
        let mut h = Harness::new();
        h.open();
        let full = h.settled_shell().height();
        h.state.query = "invoices".to_string();
        h.settle();
        let narrowed = h.settled_shell().height();
        assert!(narrowed < full);
        assert_eq!(
            narrowed,
            expected_height(SEARCH_ROW_HEIGHT + SEARCH_ROW_DESCRIPTION_HEIGHT),
            "the description matched one row"
        );
    }

    #[test]
    fn a_query_matching_nothing_shows_the_empty_state_and_no_pill() {
        let mut h = Harness::new();
        h.open();
        h.state.query = "zzz".to_string();
        h.settle();
        assert_eq!(
            h.settled_shell().height(),
            expected_height(SEARCH_EMPTY_HEIGHT)
        );
        let rec = h.read();
        assert!(
            !rec.rrects
                .iter()
                .any(|(_, _, r, _)| *r == SEARCH_ROW_RADIUS),
            "nothing selectable, so no active pill"
        );
    }

    #[test]
    fn the_arrows_walk_the_results_and_clamp_at_both_ends() {
        let mut h = Harness::new();
        h.open();
        h.focus_panel();
        let first = h.pill().y0;
        h.key(NamedKey::ArrowDown);
        let second = h.pill().y0;
        assert!(
            (second - first - (SEARCH_ROW_HEIGHT + SEARCH_ROW_DESCRIPTION_HEIGHT)).abs() < 0.001,
            "one described row down: {first} -> {second}"
        );
        for _ in 0..6 {
            h.key(NamedKey::ArrowDown);
        }
        let last = h.pill().y0;
        assert!(last > second);
        h.key(NamedKey::ArrowDown);
        assert_eq!(h.pill().y0, last, "clamped, not wrapped");
        for _ in 0..6 {
            h.key(NamedKey::ArrowUp);
        }
        assert!((h.pill().y0 - first).abs() < 0.001);
    }

    #[test]
    fn enter_selects_the_highlighted_result_and_closes_the_panel() {
        let mut h = Harness::new();
        h.open();
        h.focus_panel();
        h.key(NamedKey::ArrowDown);
        h.key(NamedKey::Enter);
        assert_eq!(h.state.selected, vec![1], "\"Billing\" is item 1");
        assert!(!h.state.open, "a selection closes the panel");
    }

    /// `EditCommand::Paste` is focus-routed exactly like `Key`/`Ime`: a
    /// clipboard paste dispatched at the panel while the field holds focus
    /// must reach it, closing the gap where a `Ctrl+V` chord's
    /// `ctx.request_paste()` succeeds but the shell's separate top-level
    /// `EditCommand::Paste(text)` dispatch it triggers is then swallowed here.
    #[test]
    fn a_paste_edit_command_reaches_the_focused_field() {
        let mut h = Harness::new();
        h.open();
        h.focus_panel();
        h.event(InputEvent::EditCommand(EditCommand::Paste(
            "billing".to_string(),
        )));
        h.settle();
        assert_eq!(
            h.state.query, "billing",
            "the pasted text must reach the focused field"
        );
    }

    /// Guard against over-forwarding: the panel's own navigation keys must
    /// still be intercepted before anything reaches the field, even while the
    /// field holds focus — `is_focus_routed()` only widens the catch-all arm
    /// *after* `handle_key`'s own interception, it must never bypass it.
    #[test]
    fn arrow_down_moves_the_highlight_not_the_field_while_focused() {
        let mut h = Harness::new();
        h.open();
        h.focus_panel();
        h.key(NamedKey::ArrowDown);
        assert_eq!(
            h.state.query, "",
            "ArrowDown must not reach the focused field as typed text"
        );
    }

    #[test]
    fn a_click_on_a_result_selects_it_and_a_release_off_it_does_not() {
        let mut h = Harness::new();
        h.open();
        let shell = h.settled_shell();
        let first_row = shell.y0 + SEARCH_HEIGHT + style::BORDER_WIDTH + SEARCH_LIST_PADDING;
        let on_first = Point::new(shell.center().x, first_row + SEARCH_ROW_HEIGHT / 2.0);
        let below = Point::new(
            on_first.x,
            first_row + SEARCH_ROW_HEIGHT + SEARCH_ROW_DESCRIPTION_HEIGHT + 4.0,
        );

        h.event(pointer(PointerPhase::Down, on_first.x, on_first.y));
        h.event(pointer(PointerPhase::Up, below.x, below.y));
        assert!(
            h.state.selected.is_empty(),
            "armed-and-re-hit, not wherever the up lands"
        );

        h.event(pointer(PointerPhase::Down, on_first.x, on_first.y));
        h.event(pointer(PointerPhase::Up, on_first.x, on_first.y));
        assert_eq!(h.state.selected, vec![0]);
        assert!(!h.state.open);
    }

    #[test]
    fn escape_closes_the_panel_and_the_shell_folds_back_into_the_trigger() {
        let mut h = Harness::new();
        h.open();
        h.focus_panel();
        h.event(escape());
        assert_eq!(h.state.opens.last(), Some(&false));
        h.step(0.0);
        // Still on screen, folding back toward the trigger's own box.
        let mid = h.read_after(80.0);
        let shell = h.shell(&mid);
        assert!(
            shell.width() < SEARCH_MAX_WIDTH,
            "the shell is shrinking: {shell:?}"
        );
        assert!(shell.width() > SEARCH_TRIGGER_WIDTH - 1.0);
    }

    #[test]
    fn the_open_panel_paints_its_chrome_and_every_row() {
        let mut h = Harness::new();
        h.open();
        let rec = h.read();
        assert!(!rec.shadows.is_empty(), "the panel casts the glass shadow");
        assert!(
            rec.strokes > 0,
            "the hairline, the search mark, the key cap"
        );
        assert!(!rec.clips.is_empty(), "the content unfolds under a clip");
        // Four titles, two descriptions and the `Esc` cap.
        assert!(
            rec.inks.len() >= 7,
            "every run was shaped and painted: {}",
            rec.inks.len()
        );
    }

    // ---- Typeface: the trigger and the panel's runs follow the theme --------

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// The trigger over the panel, both `open`. The panel's query and
    /// placeholder are empty: the wrapped baseline field has no themed-family
    /// opt-in, so its own text would paint the system face and is not what
    /// these tests pin.
    fn probe_logic(open: bool) -> impl FnMut(&mut ()) -> frust::StackView<()> {
        let anchor = OverlayAnchor::new();
        move |_: &mut ()| {
            frust::stack()
                .child(frust::Padding(
                    frust::EdgeInsets::all(INSET),
                    morphing_search_trigger::<()>(&anchor).open(open),
                ))
                .child(
                    morphing_search::<(), _, _>(
                        vec![
                            morphing_search_item("Overview").description("Dashboard and metrics"),
                            morphing_search_item("Members"),
                        ],
                        "",
                        |_: &mut (), _| {},
                        |_: &mut (), _| {},
                    )
                    .anchor(&anchor)
                    .open(open)
                    .placeholder(""),
                )
        }
    }

    #[test]
    fn search_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the trigger's text", probe_logic(false), WINDOW);
        let closed = Probe::new(probe_logic(false), WINDOW, crate::theme()).frame();
        assert_eq!(closed.len(), 2, "the trigger's placeholder and shortcut");
        assert_paints_only_in_geist("the panel's text", probe_logic(true), WINDOW);
        let open = Probe::new(probe_logic(true), WINDOW, crate::theme()).frame();
        assert_eq!(open.len(), 4, "two titles, a description and the key cap");
    }

    #[test]
    fn search_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the trigger's text", probe_logic(false), WINDOW);
        assert_follows_a_live_family_swap("the panel's text", probe_logic(true), WINDOW);
    }
}
