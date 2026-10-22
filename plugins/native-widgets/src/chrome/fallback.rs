//! Frust-drawn fallback chrome: a top bar, a toolbar row and a bottom tab row
//! built from the same [`ChromeSpec`] an arm mirrors, shown on every target without a chrome
//! arm and for every install an arm refused, so a `native_scaffold` app always
//! keeps its navigation.
//!
//! Each bar is one crate-local `View`/`Widget` pair (the refusal banner's
//! shape in `api::builders`): it shapes its own text runs, paints from the
//! ambient [`Theme`]'s roles ([`Theme::neutral`] when none is threaded, so
//! `ChromeSpec::dark` goes unread) and hit-tests its own targets. A tap is encoded with [`encode_event`] and
//! handed to [`deliver`], an arm's own report path, so the app's handlers run
//! through the scaffold's sink and never directly.
//!
//! - The top bar is 56 logical px plus the top safe-area inset, its fill under
//!   the status bar; the left/right insets pad its slots. An action with a menu
//!   toggles an inline row of its items under the bar: a leaf plugin has no
//!   overlay host, so the menu is not a popup.
//! - The tab row is 64 px plus the bottom inset; the app-confirmed `selected`
//!   tab is drawn in `primary` and decides select versus reselect.
//! - The toolbar row sits above the tab row (or takes the bottom inset when it
//!   is the lowest bar): 56 px of trailing-aligned buttons, the primary last,
//!   as a filled pill. A menu entry toggles an inline row of its items above
//!   it. Its taps are id-based ([`deliver_event`]): it holds the toolbar
//!   alone, whose indices follow the top bar's in the live spec.
//! - SF Symbol names exist only on Apple platforms, so a symbol icon draws the
//!   first letters of its name in a circle; byte icons draw untinted.
//! - No keyboard focus: each target publishes a clickable semantics node,
//!   which assistive technology activates as a tap.

use std::sync::{Arc, OnceLock};

use frust::authoring::text::{FontWeight, TextContext, TextLayout, TextStyle};
use frust::authoring::{Brush, ImageSource, teardown_child};
use frust::{Color, ColorScheme, IconData, Theme, icons};
use frust_core::accesskit::{Action, Node, Rect as AccessRect, Role};
use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerPhase, SemanticsCtx, View, Widget, any,
};
use kurbo::{Affine, BezPath, Point, Rect, Size};

use super::{
    ActionItem, ChromeEvent, ChromeGeneration, ChromeSpec, Leading, ToolbarSpec, TopBarSpec,
    deliver, deliver_event, encode_event,
};
use crate::api::{TabIcon, TabItem};

/// The top bar's content height below the top inset (M3 small top app bar).
const TOP_BAR_HEIGHT: f64 = 56.0;
/// The inline menu row's height.
const MENU_ROW_HEIGHT: f64 = 48.0;
/// A menu item chip's height inside the row.
const MENU_ITEM_HEIGHT: f64 = 40.0;
/// Horizontal padding inside a menu item chip.
const MENU_ITEM_PAD: f64 = 16.0;
/// The tab row's content height above the bottom inset.
const TAB_ROW_HEIGHT: f64 = 64.0;
/// The toolbar row's content height above the bottom inset.
const TOOLBAR_ROW_HEIGHT: f64 = 56.0;
/// The primary pill's height inside the toolbar row.
const PRIMARY_HEIGHT: f64 = 40.0;
/// A tap target's side: the 48 px minimum touch target.
const SLOT: f64 = 48.0;
/// Gap between the bar's edge (inside the insets) and its outermost slot.
const EDGE_PAD: f64 = 4.0;
/// The title's inset when there is no leading item.
const TITLE_INSET: f64 = 16.0;
/// Icon side inside a slot.
const ICON: f64 = 24.0;
/// The leading avatar's side; drawn as a circle.
const AVATAR: f64 = 32.0;
/// Horizontal padding around a text action or a leading symbol name.
const TEXT_PAD: f64 = 12.0;
/// The selected tab's indicator pill behind its icon (M3 navigation bar).
const INDICATOR: Size = Size::new(56.0, 28.0);
/// A text badge's height; an empty badge is a dot of [`BADGE_DOT`].
const BADGE_HEIGHT: f64 = 16.0;
const BADGE_DOT: f64 = 6.0;
/// The divider between a bar and the body.
const HAIRLINE: f64 = 1.0;
/// State-layer opacity over a pressed target (M3 pressed state).
const PRESSED_ALPHA: f32 = 0.12;
/// Opacity of a disabled item (M3 disabled content).
const DISABLED_ALPHA: f32 = 0.38;

// --- shared pieces -----------------------------------------------------------

/// The theme a bar paints from: the threaded one, else the neutral floor.
fn theme_or_neutral(theme: Option<&Theme>) -> &Theme {
    static NEUTRAL: OnceLock<Theme> = OnceLock::new();
    theme.unwrap_or_else(|| NEUTRAL.get_or_init(Theme::neutral))
}

fn styled(base: &TextStyle, color: Color) -> TextStyle {
    TextStyle {
        color,
        ..base.clone()
    }
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    let [r, g, b, a] = color.components;
    Color::new([r, g, b, a * alpha])
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// A symbol icon's stand-in: the first letter of up to two dot-separated
/// parts of its name, upper-cased (`"square.and.arrow.up"` is `"SA"`).
fn symbol_letters(name: &str) -> String {
    name.split('.')
        .filter_map(|part| part.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// A retained single-line text run, re-shaped only when its content or its
/// style changes. The colour is part of the style, so a theme change re-shapes.
/// The shaped half is boxed so an unshaped run stays small inside the enums.
struct Run {
    content: String,
    laid: Option<Box<(TextStyle, TextLayout)>>,
}

impl Run {
    fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            laid: None,
        }
    }

    fn set(&mut self, content: &str) {
        if self.content != content {
            self.content = content.to_owned();
            self.laid = None;
        }
    }

    fn shape(&mut self, ctx: &mut LayoutCtx, style: TextStyle) -> Size {
        if let Some(laid) = &self.laid
            && laid.0 == style
        {
            return laid.1.size();
        }
        let laid = ctx
            .text_context::<TextContext>()
            .layout(&self.content, &style, None);
        let size = laid.size();
        self.laid = Some(Box::new((style, laid)));
        size
    }

    fn size(&self) -> Size {
        self.laid.as_ref().map_or(Size::ZERO, |laid| laid.1.size())
    }

    fn paint(&self, origin: Point, scene: &mut dyn PaintScene) {
        if let Some(laid) = &self.laid {
            for run in laid.1.to_scene_runs(origin) {
                scene.draw_glyph_run(run);
            }
        }
    }

    fn paint_centered(&self, rect: Rect, scene: &mut dyn PaintScene) {
        let size = self.size();
        let origin = rect.center() - (size.to_vec2() / 2.0);
        self.paint(origin, scene);
    }
}

/// Decoded byte icons, kept across rebuilds so an unchanged icon is never
/// decoded twice. Bytes compare by content, like the spec itself.
#[derive(Default)]
struct Decoded(Vec<(Arc<[u8]>, Option<ImageSource>)>);

impl Decoded {
    fn image(&mut self, previous: &Decoded, bytes: &Arc<[u8]>) -> Option<ImageSource> {
        let same = |held: &&(Arc<[u8]>, Option<ImageSource>)| {
            Arc::ptr_eq(&held.0, bytes) || held.0[..] == bytes[..]
        };
        if let Some((_, image)) = self.0.iter().find(same) {
            return image.clone();
        }
        let image = match previous.0.iter().find(same) {
            Some((_, image)) => image.clone(),
            None => ImageSource::decode(bytes)
                .inspect_err(|error| {
                    log::debug!("frust-native-widgets: fallback chrome icon: {error}")
                })
                .ok(),
        };
        self.0.push((Arc::clone(bytes), image.clone()));
        image
    }
}

/// How one icon slot draws.
enum IconFace {
    /// Encoded bytes; `None` when they did not decode (the slot stays empty).
    Image(Option<ImageSource>),
    /// An SF Symbol's letters in a circle.
    Letters(Run),
}

impl IconFace {
    fn of(icon: &TabIcon, decoded: &mut Decoded, previous: &Decoded) -> Self {
        match icon {
            TabIcon::Bytes(bytes) => Self::Image(decoded.image(previous, bytes)),
            TabIcon::AppleSymbol(name) => Self::Letters(Run::new(symbol_letters(name))),
        }
    }

    fn shape(&mut self, ctx: &mut LayoutCtx, letters: &TextStyle) {
        if let Self::Letters(run) = self {
            run.shape(ctx, letters.clone());
        }
    }

    fn paint(&self, rect: Rect, scheme: &ColorScheme, scene: &mut dyn PaintScene) {
        match self {
            Self::Image(Some(image)) => paint_image(image, rect, scene),
            Self::Image(None) => {}
            Self::Letters(run) => {
                let side = rect.width().min(rect.height());
                let circle = Rect::from_center_size(rect.center(), (side, side));
                scene.fill_rounded_rect(
                    circle.origin(),
                    circle.size(),
                    side / 2.0,
                    scheme.secondary_container,
                );
                run.paint_centered(circle, scene);
            }
        }
    }
}

/// `image` scaled to fit inside `rect`, centred.
fn paint_image(image: &ImageSource, rect: Rect, scene: &mut dyn PaintScene) {
    let natural = image.natural_size();
    if natural.width <= 0.0 || natural.height <= 0.0 {
        return;
    }
    let scale = (rect.width() / natural.width).min(rect.height() / natural.height);
    let dest = Rect::from_center_size(rect.center(), natural * scale);
    scene.draw_image(image.image_data(), dest);
}

/// A vector icon (`design`-sized path) filled into `rect`.
fn paint_vector(path: &BezPath, design: f64, rect: Rect, color: Color, scene: &mut dyn PaintScene) {
    let side = rect.width().min(rect.height());
    let scale = if design > 0.0 { side / design } else { 1.0 };
    let origin = rect.center() - kurbo::Vec2::new(side / 2.0, side / 2.0);
    let scaled = Affine::scale(scale) * path.clone();
    scene.fill_path(origin, &scaled, &Brush::Solid(color));
}

/// What a tap on one target means.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Back,
    Action(usize),
    /// An action with a menu: toggles its inline row.
    Menu(usize),
    MenuItem(usize, usize),
    Tab(usize),
}

/// One tappable rectangle, in the bar's local coordinates.
#[derive(Clone, Copy, Debug)]
struct Target {
    rect: Rect,
    hit: Hit,
    enabled: bool,
}

/// Press tracking shared by both bars: a primary `Down` on an enabled target
/// arms it, an `Up` inside the same target fires it.
fn press(
    targets: &[Target],
    armed: &mut Option<usize>,
    ctx: &mut EventCtx,
    event: &InputEvent,
) -> (EventResult, Option<Hit>) {
    let InputEvent::Pointer(pointer) = event else {
        return (EventResult::Ignored, None);
    };
    let ignored = (EventResult::Ignored, None);
    match pointer.phase {
        PointerPhase::Down => {
            if pointer.button != PointerButton::Primary {
                return ignored;
            }
            let Some(index) = targets
                .iter()
                .position(|t| t.enabled && t.rect.contains(pointer.position))
            else {
                return ignored;
            };
            *armed = Some(index);
            ctx.capture_pointer();
            ctx.request_redraw();
            (EventResult::Handled, None)
        }
        PointerPhase::Move if armed.is_some() => (EventResult::Handled, None),
        PointerPhase::Move => ignored,
        PointerPhase::Up => {
            let Some(index) = armed.take() else {
                return ignored;
            };
            ctx.request_redraw();
            let fired = targets
                .get(index)
                .filter(|t| t.rect.contains(pointer.position))
                .map(|t| t.hit);
            (EventResult::Handled, fired)
        }
        PointerPhase::Cancel => match armed.take() {
            Some(_) => {
                ctx.request_redraw();
                (EventResult::Handled, None)
            }
            None => ignored,
        },
    }
}

/// Report `event` exactly as an arm would.
fn report(spec: &ChromeSpec, generation: ChromeGeneration, event: &ChromeEvent) {
    if let Some((kind, detail)) = encode_event(spec, event) {
        deliver(generation.get(), kind, detail);
    }
}

fn access_rect(origin: Point, rect: Rect) -> AccessRect {
    AccessRect {
        x0: origin.x + rect.x0,
        y0: origin.y + rect.y0,
        x1: origin.x + rect.x1,
        y1: origin.y + rect.y1,
    }
}

fn target_node(node: &mut Node, origin: Point, target: &Target, label: &str) {
    node.set_label(label);
    node.set_bounds(access_rect(origin, target.rect));
    if target.enabled {
        node.add_action(Action::Click);
    } else {
        node.set_disabled();
    }
}

fn pressed_layer(rect: Rect, scheme: &ColorScheme, scene: &mut dyn PaintScene) {
    scene.fill_rounded_rect(
        rect.origin(),
        rect.size(),
        rect.height().min(rect.width()) / 2.0,
        with_alpha(scheme.on_surface, PRESSED_ALPHA),
    );
}

// --- the top bar -------------------------------------------------------------

/// The fallback top bar for `spec`'s top bar, reporting under `generation`.
pub(crate) fn fallback_top_bar<Outer: 'static>(
    spec: &ChromeSpec,
    generation: ChromeGeneration,
) -> AnyView<Outer> {
    any(TopBarView {
        top: spec.top.clone().unwrap_or_default(),
        generation,
    })
}

/// Tear down a pod [`fallback_top_bar`] built.
pub(crate) fn teardown_top_bar<Outer: 'static>(pod: &mut ChildPod, ctx: &mut BuildCtx<'_>) {
    let inert = fallback_top_bar::<Outer>(&ChromeSpec::default(), ChromeGeneration(0));
    teardown_child(&inert, pod, ctx);
}

struct TopBarView {
    top: TopBarSpec,
    generation: ChromeGeneration,
}

enum LeadingFace {
    Empty,
    Back(BezPath, f64),
    Avatar(Option<ImageSource>),
    Symbol(Run),
}

enum ActionFace {
    Icon(IconFace),
    Text(Run),
}

struct TopBarWidget {
    /// The spec the bar encodes its reports against: the top bar alone.
    spec: ChromeSpec,
    generation: ChromeGeneration,
    leading: LeadingFace,
    title: Run,
    subtitle: Option<Run>,
    actions: Vec<ActionFace>,
    /// The expanded action's menu-item runs.
    menu: Vec<Run>,
    expanded: Option<usize>,
    armed: Option<usize>,
    decoded: Decoded,
    targets: Vec<Target>,
    leading_rect: Rect,
    action_rects: Vec<Rect>,
    menu_rects: Vec<Rect>,
    title_rect: Rect,
    title_origin: Point,
    subtitle_origin: Point,
    band: f64,
    size: Size,
}

impl TopBarWidget {
    fn new(generation: ChromeGeneration) -> Self {
        Self {
            spec: ChromeSpec::default(),
            generation,
            leading: LeadingFace::Empty,
            title: Run::new(""),
            subtitle: None,
            actions: Vec::new(),
            menu: Vec::new(),
            expanded: None,
            armed: None,
            decoded: Decoded::default(),
            targets: Vec::new(),
            leading_rect: Rect::ZERO,
            action_rects: Vec::new(),
            menu_rects: Vec::new(),
            title_rect: Rect::ZERO,
            title_origin: Point::ZERO,
            subtitle_origin: Point::ZERO,
            band: 0.0,
            size: Size::ZERO,
        }
    }

    fn actions(&self) -> &[ActionItem] {
        self.spec.top.as_ref().map_or(&[], |top| &top.actions)
    }

    /// Take `top` as the shown spec, reusing decoded icons.
    fn adopt(&mut self, top: &TopBarSpec) {
        let previous = std::mem::take(&mut self.decoded);
        self.leading = match &top.leading {
            Leading::None => LeadingFace::Empty,
            Leading::Back => {
                let (path, design) = IconData::from(icons::CHEVRON_LEFT).resolve();
                LeadingFace::Back(path, design)
            }
            Leading::Image(bytes) => LeadingFace::Avatar(self.decoded.image(&previous, bytes)),
            Leading::Symbol(name) => LeadingFace::Symbol(Run::new(name.clone())),
        };
        self.title.set(&top.title);
        self.subtitle = top.subtitle.as_deref().map(Run::new);
        let mut actions = Vec::with_capacity(top.actions.len());
        for action in &top.actions {
            actions.push(match &action.icon {
                Some(icon) => ActionFace::Icon(IconFace::of(icon, &mut self.decoded, &previous)),
                None => ActionFace::Text(Run::new(action.title.clone())),
            });
        }
        self.actions = actions;
        self.spec = ChromeSpec {
            top: Some(top.clone()),
            ..ChromeSpec::default()
        };
        let still_a_menu = |i: usize| top.actions.get(i).is_some_and(|a| a.menu.is_some());
        self.expanded = self.expanded.filter(|&i| still_a_menu(i));
        self.armed = None;
        self.sync_menu();
    }

    fn sync_menu(&mut self) {
        let items = self
            .expanded
            .and_then(|i| self.actions().get(i))
            .and_then(|action| action.menu.as_deref())
            .unwrap_or(&[]);
        self.menu = items
            .iter()
            .map(|item| Run::new(item.title.clone()))
            .collect();
    }

    fn fire(&mut self, hit: Hit, ctx: &mut EventCtx) {
        let event = match hit {
            Hit::Back => ChromeEvent::BackPressed,
            Hit::Action(i) => {
                let Some(action) = self.actions().get(i) else {
                    return;
                };
                ChromeEvent::ActionTapped(action.id.clone())
            }
            Hit::Menu(i) => {
                self.expanded = (self.expanded != Some(i)).then_some(i);
                self.sync_menu();
                ctx.request_layout();
                return;
            }
            Hit::MenuItem(a, m) => {
                let action = self.actions().get(a);
                let item = action.and_then(|action| action.menu.as_ref()?.get(m));
                let (Some(action), Some(item)) = (action, item) else {
                    return;
                };
                ChromeEvent::MenuItemChosen(action.id.clone(), item.id.clone())
            }
            Hit::Tab(_) => return,
        };
        if self.expanded.take().is_some() {
            self.menu.clear();
            ctx.request_layout();
        }
        report(&self.spec, self.generation, &event);
    }

    fn label(&self, hit: Hit) -> (Role, &str) {
        let actions = self.actions();
        match hit {
            Hit::Back => (Role::Button, "Back"),
            Hit::Action(i) | Hit::Menu(i) => (
                Role::Button,
                actions.get(i).map_or("", |a| a.title.as_str()),
            ),
            Hit::MenuItem(a, m) => {
                let item = actions.get(a).and_then(|a| a.menu.as_ref()?.get(m));
                (Role::MenuItem, item.map_or("", |item| item.title.as_str()))
            }
            Hit::Tab(_) => (Role::Tab, ""),
        }
    }
}

impl<Outer: 'static> View<Outer> for TopBarView {
    type Element = TopBarWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TopBarWidget {
        let mut widget = TopBarWidget::new(self.generation);
        widget.adopt(&self.top);
        widget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut TopBarWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.generation = self.generation;
        if element.spec.top.as_ref() == Some(&self.top) {
            return ChangeFlags::NONE;
        }
        element.adopt(&self.top);
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }

    fn teardown(&self, _element: &mut TopBarWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The top bar's text styles, resolved once per layout pass.
struct TopStyles {
    title: TextStyle,
    subtitle: TextStyle,
    leading: TextStyle,
    action: TextStyle,
    letters: TextStyle,
    menu: TextStyle,
    menu_destructive: TextStyle,
    menu_disabled: TextStyle,
}

impl TopStyles {
    fn of(theme: &Theme, has_subtitle: bool) -> Self {
        let (s, t) = (theme.scheme(), &theme.type_scale);
        let title = if has_subtitle {
            &t.title_medium
        } else {
            &t.title_large
        };
        Self {
            title: styled(title, s.on_surface),
            subtitle: styled(&t.body_medium, s.on_surface_variant),
            leading: styled(&t.label_large, s.on_surface),
            action: styled(&t.label_large, s.primary),
            letters: letters_style(theme),
            menu: styled(&t.label_large, s.on_surface),
            menu_destructive: styled(&t.label_large, s.error),
            menu_disabled: styled(&t.label_large, with_alpha(s.on_surface, DISABLED_ALPHA)),
        }
    }
}

fn letters_style(theme: &Theme) -> TextStyle {
    TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..styled(
            &theme.type_scale.label_small,
            theme.scheme().on_secondary_container,
        )
    }
}

impl Widget for TopBarWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let styles = TopStyles::of(
            theme_or_neutral(Theme::from_layout_ctx(ctx)),
            self.subtitle.is_some(),
        );
        let pad = ctx.window_insets().padding();
        let width = finite_or(bc.max().width, bc.min().width);
        let top = pad.top;
        let slot_y = top + (TOP_BAR_HEIGHT - SLOT) / 2.0;
        let slot = |x: f64, w: f64| Rect::new(x, slot_y, x + w, slot_y + SLOT);
        self.targets.clear();

        let mut x = pad.left + EDGE_PAD;
        let title_x0 = match &mut self.leading {
            LeadingFace::Empty => pad.left + TITLE_INSET,
            LeadingFace::Back(..) => {
                self.leading_rect = slot(x, SLOT);
                self.targets.push(Target {
                    rect: self.leading_rect,
                    hit: Hit::Back,
                    enabled: true,
                });
                x += SLOT;
                x + EDGE_PAD
            }
            LeadingFace::Avatar(_) => {
                self.leading_rect =
                    Rect::from_center_size(slot(x, SLOT).center(), (AVATAR, AVATAR));
                x += SLOT;
                x + EDGE_PAD
            }
            LeadingFace::Symbol(run) => {
                let size = run.shape(ctx, styles.leading.clone());
                let origin = (x + TEXT_PAD, top + (TOP_BAR_HEIGHT - size.height) / 2.0);
                self.leading_rect = Rect::from_origin_size(origin, size);
                x += size.width + 2.0 * TEXT_PAD;
                x + EDGE_PAD
            }
        };

        let actions = self.spec.top.as_ref().map_or(&[][..], |t| &t.actions[..]);
        let mut right = width - pad.right - EDGE_PAD;
        self.action_rects = vec![Rect::ZERO; self.actions.len()];
        for (i, face) in self.actions.iter_mut().enumerate().rev() {
            let w = match face {
                ActionFace::Icon(icon) => {
                    icon.shape(ctx, &styles.letters);
                    SLOT
                }
                ActionFace::Text(run) => {
                    (run.shape(ctx, styles.action.clone()).width + 2.0 * TEXT_PAD).max(SLOT)
                }
            };
            right -= w;
            self.action_rects[i] = slot(right, w);
        }
        for (i, action) in actions.iter().enumerate() {
            let hit = if action.menu.is_some() {
                Hit::Menu(i)
            } else {
                Hit::Action(i)
            };
            if let Some(&rect) = self.action_rects.get(i) {
                self.targets.push(Target {
                    rect,
                    hit,
                    enabled: true,
                });
            }
        }

        let title = self.title.shape(ctx, styles.title.clone());
        let subtitle = match &mut self.subtitle {
            Some(run) => run.shape(ctx, styles.subtitle.clone()),
            None => Size::ZERO,
        };
        let y = top + (TOP_BAR_HEIGHT - title.height - subtitle.height) / 2.0;
        self.title_origin = Point::new(title_x0, y);
        self.subtitle_origin = Point::new(title_x0, y + title.height);
        let title_x1 = (right - EDGE_PAD).max(title_x0);
        self.title_rect = Rect::new(title_x0, top, title_x1, top + TOP_BAR_HEIGHT);

        self.band = top + TOP_BAR_HEIGHT;
        let mut height = self.band;
        self.menu_rects.clear();
        if let Some(a) = self.expanded {
            let items = actions
                .get(a)
                .and_then(|a| a.menu.as_deref())
                .unwrap_or(&[]);
            let item_y = height + (MENU_ROW_HEIGHT - MENU_ITEM_HEIGHT) / 2.0;
            let mut item_x = pad.left + 2.0 * EDGE_PAD;
            for (m, (run, item)) in self.menu.iter_mut().zip(items).enumerate() {
                let style = match (item.enabled, item.destructive) {
                    (false, _) => &styles.menu_disabled,
                    (true, true) => &styles.menu_destructive,
                    (true, false) => &styles.menu,
                };
                let w = run.shape(ctx, style.clone()).width + 2.0 * MENU_ITEM_PAD;
                let rect = Rect::new(item_x, item_y, item_x + w, item_y + MENU_ITEM_HEIGHT);
                self.menu_rects.push(rect);
                self.targets.push(Target {
                    rect,
                    hit: Hit::MenuItem(a, m),
                    enabled: item.enabled,
                });
                item_x += w + EDGE_PAD;
            }
            height += MENU_ROW_HEIGHT;
        }
        self.size = bc.constrain(Size::new(width, height));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let scheme = theme_or_neutral(Theme::from_paint_ctx(ctx)).scheme();
        let origin = ctx.origin();
        let at = |rect: Rect| rect + origin.to_vec2();
        let width = self.size.width;
        scene.fill_rect(origin, Size::new(width, self.band), scheme.surface);
        if self.expanded.is_some() {
            scene.fill_rect(
                origin + kurbo::Vec2::new(0.0, self.band),
                Size::new(width, MENU_ROW_HEIGHT),
                scheme.surface_container_high,
            );
            for rect in &self.menu_rects {
                let r = at(*rect);
                scene.fill_rounded_rect(
                    r.origin(),
                    r.size(),
                    r.height() / 2.0,
                    scheme.surface_container_highest,
                );
            }
        }
        scene.fill_rect(
            origin + kurbo::Vec2::new(0.0, self.size.height - HAIRLINE),
            Size::new(width, HAIRLINE),
            scheme.outline_variant,
        );
        if let Some(target) = self.armed.and_then(|i| self.targets.get(i)) {
            pressed_layer(at(target.rect), scheme, scene);
        }

        let leading = at(self.leading_rect);
        match &self.leading {
            LeadingFace::Empty => {}
            LeadingFace::Back(path, design) => {
                let icon = Rect::from_center_size(leading.center(), (ICON, ICON));
                paint_vector(path, *design, icon, scheme.on_surface, scene);
            }
            LeadingFace::Avatar(image) => {
                scene.push_clip_rounded(leading.origin(), leading.size(), AVATAR / 2.0);
                match image {
                    Some(image) => paint_image(image, leading, scene),
                    None => scene.fill_rect(
                        leading.origin(),
                        leading.size(),
                        scheme.surface_container_highest,
                    ),
                }
                scene.pop_clip();
            }
            LeadingFace::Symbol(run) => run.paint(leading.origin(), scene),
        }

        let title = at(self.title_rect);
        scene.push_clip(title.origin(), title.size());
        self.title
            .paint(origin + self.title_origin.to_vec2(), scene);
        if let Some(subtitle) = &self.subtitle {
            subtitle.paint(origin + self.subtitle_origin.to_vec2(), scene);
        }
        scene.pop_clip();

        for (face, rect) in self.actions.iter().zip(&self.action_rects) {
            let rect = at(*rect);
            match face {
                ActionFace::Icon(icon) => {
                    let side = if matches!(icon, IconFace::Letters(_)) {
                        ICON + 4.0
                    } else {
                        ICON
                    };
                    icon.paint(
                        Rect::from_center_size(rect.center(), (side, side)),
                        scheme,
                        scene,
                    );
                }
                ActionFace::Text(run) => run.paint_centered(rect, scene),
            }
        }
        for (run, rect) in self.menu.iter().zip(&self.menu_rects) {
            run.paint_centered(at(*rect), scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let (result, fired) = press(&self.targets, &mut self.armed, ctx, event);
        if let Some(hit) = fired {
            self.fire(hit, ctx);
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let origin = ctx.origin();
        let top = self.spec.top.as_ref();
        let title = top.map_or("", |top| top.title.as_str());
        ctx.push_container(
            Role::Toolbar,
            |node| node.set_label(title),
            |ctx| {
                ctx.push_node(Role::Heading, |node| {
                    node.set_label(title);
                    node.set_bounds(access_rect(origin, self.title_rect));
                });
                if let Some(subtitle) = top.and_then(|top| top.subtitle.as_deref()) {
                    ctx.push_node(Role::Label, |node| node.set_label(subtitle));
                }
                for target in &self.targets {
                    let (role, label) = self.label(target.hit);
                    ctx.push_node(role, |node| {
                        target_node(node, origin, target, label);
                        if let Hit::Menu(i) = target.hit {
                            node.set_expanded(self.expanded == Some(i));
                        }
                    });
                }
            },
        );
    }
}

// --- the toolbar row ---------------------------------------------------------

/// The fallback toolbar row for `spec`'s toolbar, reporting under `generation`.
pub(crate) fn fallback_toolbar_row<Outer: 'static>(
    spec: &ChromeSpec,
    generation: ChromeGeneration,
) -> AnyView<Outer> {
    any(ToolbarRowView {
        toolbar: spec.toolbar.clone().unwrap_or_default(),
        generation,
    })
}

/// Tear down a pod [`fallback_toolbar_row`] built.
pub(crate) fn teardown_toolbar_row<Outer: 'static>(pod: &mut ChildPod, ctx: &mut BuildCtx<'_>) {
    let inert = fallback_toolbar_row::<Outer>(&ChromeSpec::default(), ChromeGeneration(0));
    teardown_child(&inert, pod, ctx);
}

struct ToolbarRowView {
    toolbar: ToolbarSpec,
    generation: ChromeGeneration,
}

struct ToolbarRowWidget {
    toolbar: ToolbarSpec,
    generation: ChromeGeneration,
    /// One face per entry, items then the primary.
    faces: Vec<ActionFace>,
    /// The expanded entry's menu-item runs.
    menu: Vec<Run>,
    expanded: Option<usize>,
    armed: Option<usize>,
    decoded: Decoded,
    targets: Vec<Target>,
    rects: Vec<Rect>,
    menu_rects: Vec<Rect>,
    size: Size,
}

impl ToolbarRowWidget {
    fn new(generation: ChromeGeneration) -> Self {
        Self {
            toolbar: ToolbarSpec::default(),
            generation,
            faces: Vec::new(),
            menu: Vec::new(),
            expanded: None,
            armed: None,
            decoded: Decoded::default(),
            targets: Vec::new(),
            rects: Vec::new(),
            menu_rects: Vec::new(),
            size: Size::ZERO,
        }
    }

    fn entry(&self, index: usize) -> Option<&ActionItem> {
        self.toolbar.entries().nth(index)
    }

    fn is_primary(&self, index: usize) -> bool {
        self.toolbar.primary.is_some() && index == self.toolbar.items.len()
    }

    /// Take `toolbar` as the shown one, reusing decoded icons.
    fn adopt(&mut self, toolbar: &ToolbarSpec) {
        let previous = std::mem::take(&mut self.decoded);
        self.faces = toolbar
            .entries()
            .map(|entry| match &entry.icon {
                Some(icon) => ActionFace::Icon(IconFace::of(icon, &mut self.decoded, &previous)),
                None => ActionFace::Text(Run::new(entry.title.clone())),
            })
            .collect();
        self.toolbar = toolbar.clone();
        self.expanded = self
            .expanded
            .filter(|&i| self.entry(i).is_some_and(|entry| entry.menu.is_some()));
        self.armed = None;
        self.sync_menu();
    }

    fn sync_menu(&mut self) {
        let items = self
            .expanded
            .and_then(|i| self.entry(i))
            .and_then(|entry| entry.menu.as_deref())
            .unwrap_or(&[]);
        self.menu = items
            .iter()
            .map(|item| Run::new(item.title.clone()))
            .collect();
    }

    fn fire(&mut self, hit: Hit, ctx: &mut EventCtx) {
        let event = match hit {
            Hit::Action(i) => {
                let Some(entry) = self.entry(i) else {
                    return;
                };
                ChromeEvent::ActionTapped(entry.id.clone())
            }
            Hit::Menu(i) => {
                self.expanded = (self.expanded != Some(i)).then_some(i);
                self.sync_menu();
                ctx.request_layout();
                return;
            }
            Hit::MenuItem(a, m) => {
                let entry = self.entry(a);
                let item = entry.and_then(|entry| entry.menu.as_ref()?.get(m));
                let (Some(entry), Some(item)) = (entry, item) else {
                    return;
                };
                ChromeEvent::MenuItemChosen(entry.id.clone(), item.id.clone())
            }
            Hit::Back | Hit::Tab(_) => return,
        };
        if self.expanded.take().is_some() {
            self.menu.clear();
            ctx.request_layout();
        }
        deliver_event(self.generation.get(), &event);
    }

    fn label(&self, hit: Hit) -> (Role, &str) {
        match hit {
            Hit::Action(i) | Hit::Menu(i) => (
                Role::Button,
                self.entry(i).map_or("", |entry| entry.title.as_str()),
            ),
            Hit::MenuItem(a, m) => {
                let item = self.entry(a).and_then(|entry| entry.menu.as_ref()?.get(m));
                (Role::MenuItem, item.map_or("", |item| item.title.as_str()))
            }
            Hit::Back | Hit::Tab(_) => (Role::Button, ""),
        }
    }
}

impl<Outer: 'static> View<Outer> for ToolbarRowView {
    type Element = ToolbarRowWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ToolbarRowWidget {
        let mut widget = ToolbarRowWidget::new(self.generation);
        widget.adopt(&self.toolbar);
        widget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ToolbarRowWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.generation = self.generation;
        if element.toolbar == self.toolbar {
            return ChangeFlags::NONE;
        }
        element.adopt(&self.toolbar);
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }

    fn teardown(&self, _element: &mut ToolbarRowWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for ToolbarRowWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = theme_or_neutral(Theme::from_layout_ctx(ctx));
        let styles = TopStyles::of(theme, false);
        let primary_style = styled(&theme.type_scale.label_large, theme.scheme().on_primary);
        let pad = ctx.window_insets().padding();
        let width = finite_or(bc.max().width, bc.min().width);
        self.targets.clear();
        self.menu_rects.clear();

        let mut y = 0.0;
        if let Some(a) = self.expanded {
            let items = self
                .toolbar
                .entries()
                .nth(a)
                .and_then(|entry| entry.menu.as_deref())
                .unwrap_or(&[]);
            let item_y = (MENU_ROW_HEIGHT - MENU_ITEM_HEIGHT) / 2.0;
            let mut item_x = pad.left + 2.0 * EDGE_PAD;
            for (m, (run, item)) in self.menu.iter_mut().zip(items).enumerate() {
                let style = match (item.enabled, item.destructive) {
                    (false, _) => &styles.menu_disabled,
                    (true, true) => &styles.menu_destructive,
                    (true, false) => &styles.menu,
                };
                let w = run.shape(ctx, style.clone()).width + 2.0 * MENU_ITEM_PAD;
                let rect = Rect::new(item_x, item_y, item_x + w, item_y + MENU_ITEM_HEIGHT);
                self.menu_rects.push(rect);
                self.targets.push(Target {
                    rect,
                    hit: Hit::MenuItem(a, m),
                    enabled: item.enabled,
                });
                item_x += w + EDGE_PAD;
            }
            y = MENU_ROW_HEIGHT;
        }

        let primary = self
            .toolbar
            .primary
            .is_some()
            .then_some(self.toolbar.items.len());
        let mut right = width - pad.right - EDGE_PAD;
        self.rects = vec![Rect::ZERO; self.faces.len()];
        for (i, face) in self.faces.iter_mut().enumerate().rev() {
            let style = if primary == Some(i) {
                &primary_style
            } else {
                &styles.action
            };
            let w = match face {
                ActionFace::Icon(icon) => {
                    icon.shape(ctx, &styles.letters);
                    SLOT
                }
                ActionFace::Text(run) => {
                    (run.shape(ctx, style.clone()).width + 2.0 * TEXT_PAD).max(SLOT)
                }
            };
            let h = if primary == Some(i) {
                PRIMARY_HEIGHT
            } else {
                SLOT
            };
            right -= w;
            let top = y + (TOOLBAR_ROW_HEIGHT - h) / 2.0;
            self.rects[i] = Rect::new(right, top, right + w, top + h);
            right -= EDGE_PAD;
        }
        for (i, rect) in self.rects.iter().enumerate() {
            let menu = self
                .toolbar
                .entries()
                .nth(i)
                .is_some_and(|e| e.menu.is_some());
            self.targets.push(Target {
                rect: *rect,
                hit: if menu { Hit::Menu(i) } else { Hit::Action(i) },
                enabled: true,
            });
        }
        self.size = bc.constrain(Size::new(width, y + TOOLBAR_ROW_HEIGHT + pad.bottom));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let scheme = theme_or_neutral(Theme::from_paint_ctx(ctx)).scheme();
        let origin = ctx.origin();
        let at = |rect: Rect| rect + origin.to_vec2();
        scene.fill_rect(origin, self.size, scheme.surface_container);
        if self.expanded.is_some() {
            scene.fill_rect(
                origin,
                Size::new(self.size.width, MENU_ROW_HEIGHT),
                scheme.surface_container_high,
            );
            for rect in &self.menu_rects {
                let r = at(*rect);
                let fill = scheme.surface_container_highest;
                scene.fill_rounded_rect(r.origin(), r.size(), r.height() / 2.0, fill);
            }
        }
        scene.fill_rect(
            origin,
            Size::new(self.size.width, HAIRLINE),
            scheme.outline_variant,
        );
        for (i, rect) in self.rects.iter().enumerate() {
            if self.is_primary(i) {
                let r = at(*rect);
                scene.fill_rounded_rect(r.origin(), r.size(), r.height() / 2.0, scheme.primary);
            }
        }
        if let Some(target) = self.armed.and_then(|i| self.targets.get(i)) {
            pressed_layer(at(target.rect), scheme, scene);
        }
        for (face, rect) in self.faces.iter().zip(&self.rects) {
            let rect = at(*rect);
            match face {
                ActionFace::Icon(icon) => {
                    let side = if matches!(icon, IconFace::Letters(_)) {
                        ICON + 4.0
                    } else {
                        ICON
                    };
                    icon.paint(
                        Rect::from_center_size(rect.center(), (side, side)),
                        scheme,
                        scene,
                    );
                }
                ActionFace::Text(run) => run.paint_centered(rect, scene),
            }
        }
        for (run, rect) in self.menu.iter().zip(&self.menu_rects) {
            run.paint_centered(at(*rect), scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let (result, fired) = press(&self.targets, &mut self.armed, ctx, event);
        if let Some(hit) = fired {
            self.fire(hit, ctx);
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let origin = ctx.origin();
        ctx.push_container(
            Role::Toolbar,
            |_| {},
            |ctx| {
                for target in &self.targets {
                    let (role, label) = self.label(target.hit);
                    ctx.push_node(role, |node| {
                        target_node(node, origin, target, label);
                        if let Hit::Menu(i) = target.hit {
                            node.set_expanded(self.expanded == Some(i));
                        }
                    });
                }
            },
        );
    }
}

// --- the tab row -------------------------------------------------------------

/// What one tab cell shows.
#[derive(Clone, Debug, PartialEq)]
struct TabFace {
    title: String,
    icon: Option<TabIcon>,
    selected_icon: Option<TabIcon>,
    badge: Option<String>,
    enabled: bool,
}

/// `item`'s face: its real title, icons, badge and enabled state.
fn tab_face(item: &TabItem) -> TabFace {
    TabFace {
        title: item.title().to_owned(),
        icon: Some(item.icon().clone()),
        selected_icon: item.selected_icon_ref().cloned(),
        badge: item.badge_text().map(str::to_owned),
        enabled: item.is_enabled(),
    }
}

/// The fallback tab row for `spec`'s tabs, reporting under `generation`.
pub(crate) fn fallback_tab_row<Outer: 'static>(
    spec: &ChromeSpec,
    generation: ChromeGeneration,
) -> AnyView<Outer> {
    any(TabRowView {
        spec: ChromeSpec {
            tabs: spec.tabs.clone(),
            selected: spec.selected.clone(),
            ..ChromeSpec::default()
        },
        faces: spec.tabs.iter().map(tab_face).collect(),
        generation,
    })
}

/// Tear down a pod [`fallback_tab_row`] built.
pub(crate) fn teardown_tab_row<Outer: 'static>(pod: &mut ChildPod, ctx: &mut BuildCtx<'_>) {
    let inert = fallback_tab_row::<Outer>(&ChromeSpec::default(), ChromeGeneration(0));
    teardown_child(&inert, pod, ctx);
}

struct TabRowView {
    /// The tabs and the confirmed selection alone.
    spec: ChromeSpec,
    faces: Vec<TabFace>,
    generation: ChromeGeneration,
}

struct TabCell {
    title: Run,
    icon: Option<IconFace>,
    selected_icon: Option<IconFace>,
    badge: Option<Run>,
    enabled: bool,
    icon_rect: Option<Rect>,
    title_origin: Point,
    badge_rect: Option<Rect>,
}

struct TabRowWidget {
    spec: ChromeSpec,
    faces: Vec<TabFace>,
    generation: ChromeGeneration,
    cells: Vec<TabCell>,
    selected: Option<usize>,
    armed: Option<usize>,
    decoded: Decoded,
    targets: Vec<Target>,
    size: Size,
}

impl TabRowWidget {
    fn adopt(&mut self, view: &TabRowView) {
        let previous = std::mem::take(&mut self.decoded);
        let mut cells = Vec::with_capacity(view.faces.len());
        for face in &view.faces {
            let mut icon = |icon: &Option<TabIcon>| {
                icon.as_ref()
                    .map(|icon| IconFace::of(icon, &mut self.decoded, &previous))
            };
            cells.push(TabCell {
                title: Run::new(face.title.clone()),
                icon: icon(&face.icon),
                selected_icon: icon(&face.selected_icon),
                badge: face.badge.as_deref().map(Run::new),
                enabled: face.enabled,
                icon_rect: None,
                title_origin: Point::ZERO,
                badge_rect: None,
            });
        }
        self.cells = cells;
        self.spec = view.spec.clone();
        self.faces = view.faces.clone();
        self.selected = self
            .spec
            .selected
            .as_ref()
            .and_then(|id| self.spec.tabs.iter().position(|tab| tab.id() == id));
        self.armed = None;
    }

    fn fire(&self, index: usize) {
        let Some(id) = self.spec.tabs.get(index).map(|tab| tab.id().clone()) else {
            return;
        };
        let event = if self.selected == Some(index) {
            ChromeEvent::TabReselected(id)
        } else {
            ChromeEvent::TabSelected(id)
        };
        report(&self.spec, self.generation, &event);
    }
}

impl<Outer: 'static> View<Outer> for TabRowView {
    type Element = TabRowWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> TabRowWidget {
        let mut widget = TabRowWidget {
            spec: ChromeSpec::default(),
            faces: Vec::new(),
            generation: self.generation,
            cells: Vec::new(),
            selected: None,
            armed: None,
            decoded: Decoded::default(),
            targets: Vec::new(),
            size: Size::ZERO,
        };
        widget.adopt(self);
        widget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut TabRowWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.generation = self.generation;
        if element.spec == self.spec && element.faces == self.faces {
            return ChangeFlags::NONE;
        }
        element.adopt(self);
        ChangeFlags::LAYOUT | ChangeFlags::PAINT
    }

    fn teardown(&self, _element: &mut TabRowWidget, _ctx: &mut BuildCtx<'_>) {}
}

impl Widget for TabRowWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let (label, label_selected, badge, letters) = {
            let theme = theme_or_neutral(Theme::from_layout_ctx(ctx));
            let (s, t) = (theme.scheme(), &theme.type_scale);
            (
                styled(&t.label_medium, s.on_surface_variant),
                styled(&t.label_medium, s.primary),
                styled(&t.label_small, s.on_error),
                letters_style(theme),
            )
        };
        let pad = ctx.window_insets().padding();
        let width = finite_or(bc.max().width, bc.min().width);
        let count = self.cells.len().max(1) as f64;
        let cell_width = ((width - pad.left - pad.right) / count).max(0.0);
        self.targets.clear();
        for (i, cell) in self.cells.iter_mut().enumerate() {
            let x0 = pad.left + i as f64 * cell_width;
            let rect = Rect::new(x0, 0.0, x0 + cell_width, TAB_ROW_HEIGHT);
            let style = if self.selected == Some(i) {
                &label_selected
            } else {
                &label
            };
            let title = cell.title.shape(ctx, style.clone());
            let has_icon = cell.icon.is_some() || cell.selected_icon.is_some();
            for face in [&mut cell.icon, &mut cell.selected_icon]
                .into_iter()
                .flatten()
            {
                face.shape(ctx, &letters);
            }
            let center_x = rect.center().x;
            let title_y = if has_icon {
                cell.icon_rect = Some(Rect::from_center_size((center_x, 20.0), (ICON, ICON)));
                38.0
            } else {
                cell.icon_rect = None;
                (TAB_ROW_HEIGHT - title.height) / 2.0
            };
            cell.title_origin = Point::new(center_x - title.width / 2.0, title_y);
            let anchor = cell
                .icon_rect
                .unwrap_or_else(|| Rect::from_origin_size(cell.title_origin, title));
            cell.badge_rect = cell.badge.as_mut().map(|run| {
                let text = run.shape(ctx, badge.clone());
                if run.content.is_empty() {
                    Rect::from_center_size((anchor.x1, anchor.y0 + 2.0), (BADGE_DOT, BADGE_DOT))
                } else {
                    let w = (text.width + 8.0).max(BADGE_HEIGHT);
                    let x = anchor.x1 - 6.0;
                    let y = anchor.y0 - 4.0;
                    Rect::new(x, y, x + w, y + BADGE_HEIGHT)
                }
            });
            self.targets.push(Target {
                rect,
                hit: Hit::Tab(i),
                enabled: cell.enabled,
            });
        }
        self.size = bc.constrain(Size::new(width, TAB_ROW_HEIGHT + pad.bottom));
        self.size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let scheme = theme_or_neutral(Theme::from_paint_ctx(ctx)).scheme();
        let origin = ctx.origin();
        let at = |rect: Rect| rect + origin.to_vec2();
        scene.fill_rect(origin, self.size, scheme.surface_container);
        scene.fill_rect(
            origin,
            Size::new(self.size.width, HAIRLINE),
            scheme.outline_variant,
        );
        for (i, (cell, target)) in self.cells.iter().zip(&self.targets).enumerate() {
            let cell_rect = at(target.rect);
            if !cell.enabled {
                scene.push_layer(cell_rect.origin(), cell_rect.size(), DISABLED_ALPHA);
            }
            let selected = self.selected == Some(i);
            let pill = cell
                .icon_rect
                .map(|icon| Rect::from_center_size(at(icon).center(), INDICATOR));
            if let (true, Some(pill)) = (selected, pill) {
                let radius = pill.height() / 2.0;
                scene.fill_rounded_rect(
                    pill.origin(),
                    pill.size(),
                    radius,
                    scheme.secondary_container,
                );
            }
            if self.armed == Some(i) {
                pressed_layer(pill.unwrap_or(cell_rect), scheme, scene);
            }
            let face = match (selected, &cell.selected_icon) {
                (true, Some(face)) => Some(face),
                _ => cell.icon.as_ref(),
            };
            if let (Some(face), Some(icon)) = (face, cell.icon_rect) {
                face.paint(at(icon), scheme, scene);
            }
            cell.title
                .paint(origin + cell.title_origin.to_vec2(), scene);
            if let (Some(run), Some(rect)) = (&cell.badge, cell.badge_rect) {
                let rect = at(rect);
                scene.fill_rounded_rect(
                    rect.origin(),
                    rect.size(),
                    rect.height() / 2.0,
                    scheme.error,
                );
                if !run.content.is_empty() {
                    run.paint_centered(rect, scene);
                }
            }
            if !cell.enabled {
                scene.pop_layer();
            }
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let (result, fired) = press(&self.targets, &mut self.armed, ctx, event);
        if let Some(Hit::Tab(index)) = fired {
            self.fire(index);
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let origin = ctx.origin();
        ctx.push_container(
            Role::TabList,
            |_| {},
            |ctx| {
                for (i, (face, target)) in self.faces.iter().zip(&self.targets).enumerate() {
                    ctx.push_node(Role::Tab, |node| {
                        target_node(node, origin, target, &face.title);
                        node.set_selected(self.selected == Some(i));
                        if let Some(badge) = &face.badge {
                            node.set_description(badge.as_str());
                        }
                    });
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Mutex;

    use frust::Brightness;
    use frust_core::{FrameTime, PointerEvent, RenderRoot, WindowEdgeInsets, WindowInsets};
    use frust_scene::GlyphRun;

    use super::*;
    use crate::api::{MenuItem, NativeScaffoldView, TabId, native_scaffold};
    use crate::chrome::slot_test_guard;

    const SIZES: [(f64, f64); 2] = [(390.0, 844.0), (900.0, 700.0)];
    const BRIGHTNESSES: [Brightness; 2] = [Brightness::Dark, Brightness::Light];

    /// What the body saw: its painted rect and the insets its layout read.
    #[derive(Clone, Copy, Debug, Default)]
    struct Seen {
        origin: Point,
        size: Size,
        top: f64,
        bottom: f64,
        left: f64,
    }

    #[derive(Clone)]
    struct Probe(Rc<RefCell<Seen>>);

    struct ProbeWidget(Rc<RefCell<Seen>>);

    impl<S: 'static> View<S> for Probe {
        type Element = ProbeWidget;

        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget(Rc::clone(&self.0))
        }

        fn rebuild(&self, _: &Self, _: &mut ProbeWidget, _: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }

        fn teardown(&self, _: &mut ProbeWidget, _: &mut BuildCtx<'_>) {}
    }

    impl Widget for ProbeWidget {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            let pad = ctx.window_insets().padding();
            let mut seen = self.0.borrow_mut();
            (seen.top, seen.bottom, seen.left) = (pad.top, pad.bottom, pad.left);
            Size::new(
                finite_or(bc.max().width, 0.0),
                finite_or(bc.max().height, 0.0),
            )
        }

        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            let mut seen = self.0.borrow_mut();
            (seen.origin, seen.size) = (ctx.origin(), ctx.size());
        }
    }

    /// Records text runs, every solid colour painted, and images.
    #[derive(Default)]
    struct Rec {
        glyph_runs: usize,
        colors: Vec<[f32; 4]>,
        images: usize,
    }

    impl PaintScene for Rec {
        fn fill_rect(&mut self, _: Point, _: Size, color: Color) {
            self.colors.push(color.components);
        }
        fn draw_text(&mut self, _: Point, _: &str) {}
        fn fill_rounded_rect(&mut self, _: Point, _: Size, _: f64, color: Color) {
            self.colors.push(color.components);
        }
        fn draw_glyph_run(&mut self, run: GlyphRun) {
            self.glyph_runs += 1;
            if let Brush::Solid(color) = run.brush {
                self.colors.push(color.components);
            }
        }
        fn fill_path(&mut self, _: Point, _: &BezPath, brush: &Brush) {
            if let Brush::Solid(color) = brush {
                self.colors.push(color.components);
            }
        }
        fn draw_image(&mut self, _: &frust::authoring::ImageData, _: Rect) {
            self.images += 1;
        }
    }

    type Log = Arc<Mutex<Vec<String>>>;

    fn scaffold(seen: &Rc<RefCell<Seen>>, log: &Log) -> NativeScaffoldView<Probe> {
        let note = |tag: &'static str| {
            let log = Arc::clone(log);
            move |detail: String| log.lock().unwrap().push(format!("{tag}:{detail}"))
        };
        let (select, reselect, action, menu, back) = (
            note("select"),
            note("reselect"),
            note("action"),
            note("menu"),
            note("back"),
        );
        native_scaffold(Probe(Rc::clone(seen)))
            .tabs(
                vec![
                    TabItem::new("home", "Home", TabIcon::AppleSymbol("house".into())),
                    TabItem::new("inbox", "Inbox", TabIcon::AppleSymbol("tray".into())),
                ],
                "home",
            )
            .title("Inbox")
            .subtitle("3 unread")
            .leading(Leading::Back)
            .actions(vec![
                ActionItem::new("add", "Add").icon(TabIcon::AppleSymbol("plus".into())),
                ActionItem::new("more", "More")
                    .icon(TabIcon::AppleSymbol("ellipsis".into()))
                    .menu(vec![
                        MenuItem::new("share", "Share"),
                        MenuItem::new("delete", "Delete").destructive(true),
                    ]),
            ])
            .on_tab_select(move |id| select(id.to_string()))
            .on_tab_reselect(move |id| reselect(id.to_string()))
            .on_action(move |id| action(id.to_string()))
            .on_menu(move |a, m| menu(format!("{a}/{m}")))
            .on_back(move || back(String::new()))
    }

    /// One app over a headless `RenderRoot`, the desktop shell's own seam.
    struct App {
        root: RenderRoot<(), NativeScaffoldView<Probe>>,
        seen: Rc<RefCell<Seen>>,
        log: Log,
        text: TextContext,
        size: Size,
        toolbar: Option<ToolbarSpec>,
        tabs: bool,
    }

    impl App {
        /// A fresh app owning the slot: a `RenderRoot` never tears its tree
        /// down, so an earlier app's generation is dropped here instead.
        fn new(size: (f64, f64), brightness: Brightness, insets: WindowInsets) -> Self {
            crate::chrome::lock_slot().clear();
            let mut root = RenderRoot::new();
            root.set_theme(Box::new(Theme::neutral().with_brightness(brightness)));
            root.set_insets(insets);
            Self {
                root,
                seen: Rc::default(),
                log: Arc::default(),
                text: TextContext::new(),
                size: Size::new(size.0, size.1),
                toolbar: None,
                tabs: true,
            }
        }

        /// This app with `toolbar`, and without tabs unless `tabs`.
        fn with_toolbar(mut self, toolbar: ToolbarSpec, tabs: bool) -> Self {
            self.toolbar = Some(toolbar);
            self.tabs = tabs;
            self
        }

        fn frame(&mut self) -> Rec {
            let (seen, log) = (Rc::clone(&self.seen), Arc::clone(&self.log));
            let (toolbar, tabs) = (self.toolbar.clone(), self.tabs);
            let mut build = move |_: &mut ()| {
                let view = scaffold(&seen, &log);
                let view = if tabs {
                    view
                } else {
                    view.tabs(Vec::new(), "")
                };
                match &toolbar {
                    Some(toolbar) => view.toolbar(toolbar.clone()),
                    None => view,
                }
            };
            self.root.rebuild(&mut build, &mut ());
            self.root.layout_with_text(self.size, &mut self.text);
            let mut rec = Rec::default();
            self.root.paint(&mut rec, FrameTime::from_nanos(0));
            rec
        }

        fn tap(&mut self, x: f64, y: f64) {
            for phase in [PointerPhase::Down, PointerPhase::Up] {
                let event = InputEvent::Pointer(PointerEvent {
                    phase,
                    position: Point::new(x, y),
                    button: PointerButton::Primary,
                });
                self.root.event(&mut (), &event);
            }
            self.frame();
        }

        fn seen(&self) -> Seen {
            *self.seen.borrow()
        }

        fn take_log(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }

        fn nodes(&self, role: Role) -> Vec<(frust_core::accesskit::NodeId, Node)> {
            let update = self.root.semantics();
            update
                .nodes
                .into_iter()
                .filter(|(_, node)| node.role() == role)
                .collect()
        }
    }

    fn insets(left: f64, top: f64, bottom: f64) -> WindowInsets {
        WindowInsets::new(
            WindowEdgeInsets::new(left, top, 0.0, bottom),
            WindowEdgeInsets::ZERO,
        )
    }

    #[test]
    fn the_bars_lay_out_and_paint_at_both_sizes_under_both_brightnesses() {
        let _guard = slot_test_guard();
        for size in SIZES {
            for brightness in BRIGHTNESSES {
                let mut app = App::new(size, brightness, WindowInsets::default());
                let rec = app.frame();
                // Title, subtitle, the two actions' letters, the two tab labels.
                assert!(
                    rec.glyph_runs >= 6,
                    "{size:?} {brightness:?}: {}",
                    rec.glyph_runs
                );
                let seen = app.seen();
                assert_eq!(seen.origin, Point::new(0.0, TOP_BAR_HEIGHT));
                assert_eq!(seen.size.height, size.1 - TOP_BAR_HEIGHT - TAB_ROW_HEIGHT);
                assert_eq!(seen.size.width, size.0);
            }
        }
    }

    #[test]
    fn the_bars_consume_the_top_and_bottom_insets_once() {
        let _guard = slot_test_guard();
        let mut bare = App::new(SIZES[0], Brightness::Light, WindowInsets::default());
        bare.frame();
        let mut inset = App::new(SIZES[0], Brightness::Light, insets(12.0, 47.0, 34.0));
        inset.frame();
        let (bare, inset) = (bare.seen(), inset.seen());
        assert_eq!(
            inset.origin.y - bare.origin.y,
            47.0,
            "the top bar grew by the inset"
        );
        assert_eq!(bare.size.height - inset.size.height, 47.0 + 34.0);
        assert_eq!(
            (inset.top, inset.bottom),
            (0.0, 0.0),
            "the body reads them consumed"
        );
        assert_eq!(inset.left, 12.0, "side insets reach the body");
    }

    #[test]
    fn tab_taps_report_select_or_reselect_against_the_confirmed_tab() {
        let _guard = slot_test_guard();
        let (w, h) = SIZES[1];
        let mut app = App::new((w, h), Brightness::Light, WindowInsets::default());
        app.frame();
        let row_y = h - TAB_ROW_HEIGHT / 2.0;
        app.tap(w * 0.75, row_y);
        app.tap(w * 0.25, row_y);
        assert_eq!(app.take_log(), ["select:inbox", "reselect:home"]);
    }

    #[test]
    fn back_and_plain_actions_report_through_the_sink() {
        let _guard = slot_test_guard();
        let (w, _) = SIZES[0];
        let mut app = App::new(SIZES[0], Brightness::Dark, WindowInsets::default());
        app.frame();
        let y = TOP_BAR_HEIGHT / 2.0;
        app.tap(EDGE_PAD + SLOT / 2.0, y);
        app.tap(w - EDGE_PAD - SLOT - SLOT / 2.0, y);
        assert_eq!(app.take_log(), ["back:", "action:add"]);
    }

    #[test]
    fn a_menu_action_expands_its_items_inline_and_reports_the_choice() {
        let _guard = slot_test_guard();
        let (w, _) = SIZES[0];
        let mut app = App::new(SIZES[0], Brightness::Light, WindowInsets::default());
        app.frame();
        assert!(app.nodes(Role::MenuItem).is_empty());
        app.tap(w - EDGE_PAD - SLOT / 2.0, TOP_BAR_HEIGHT / 2.0);
        assert!(
            app.take_log().is_empty(),
            "a menu action reports nothing itself"
        );
        assert_eq!(app.seen().origin.y, TOP_BAR_HEIGHT + MENU_ROW_HEIGHT);
        assert_eq!(app.nodes(Role::MenuItem).len(), 2);
        app.tap(
            2.0 * EDGE_PAD + MENU_ITEM_PAD,
            TOP_BAR_HEIGHT + MENU_ROW_HEIGHT / 2.0,
        );
        assert_eq!(app.take_log(), ["menu:more/share"]);
        assert_eq!(
            app.seen().origin.y,
            TOP_BAR_HEIGHT,
            "a choice collapses the row"
        );
    }

    #[test]
    fn colours_follow_the_theme_brightness() {
        let _guard = slot_test_guard();
        let paint = |brightness| App::new(SIZES[0], brightness, WindowInsets::default()).frame();
        let (dark, light) = (paint(Brightness::Dark), paint(Brightness::Light));
        assert_eq!(dark.glyph_runs, light.glyph_runs);
        assert_ne!(dark.colors, light.colors);
    }

    #[test]
    fn an_accessibility_click_on_a_tab_reports_like_a_tap() {
        let _guard = slot_test_guard();
        let mut app = App::new(SIZES[0], Brightness::Light, WindowInsets::default());
        app.frame();
        let tabs = app.nodes(Role::Tab);
        assert_eq!(tabs.len(), 2);
        let (inbox, _) = tabs
            .iter()
            .find(|(_, node)| node.label() == Some("Inbox"))
            .expect("the inbox tab node");
        assert!(
            tabs.iter()
                .any(|(_, node)| node.is_selected() == Some(true))
        );
        app.root
            .perform_accessibility_action(&mut (), *inbox, Action::Click);
        assert_eq!(app.take_log(), ["select:inbox"]);
    }

    fn comment_toolbar() -> ToolbarSpec {
        ToolbarSpec::new()
            .item(ActionItem::new("info", "Info"))
            .item(ActionItem::new("sort", "Sort").menu(vec![
                MenuItem::new("new", "Newest"),
                MenuItem::new("old", "Oldest"),
            ]))
            .primary(ActionItem::new("comment", "Comment"))
    }

    #[test]
    fn the_toolbar_row_sits_above_the_tab_row_and_reports_by_id() {
        let _guard = slot_test_guard();
        let (w, h) = SIZES[0];
        let mut app = App::new(SIZES[0], Brightness::Light, insets(0.0, 0.0, 34.0))
            .with_toolbar(comment_toolbar(), true);
        app.frame();
        let seen = app.seen();
        let tab_row = TAB_ROW_HEIGHT + 34.0;
        assert_eq!(
            seen.size.height,
            h - TOP_BAR_HEIGHT - TOOLBAR_ROW_HEIGHT - tab_row
        );
        assert_eq!(seen.bottom, 0.0, "the bottom inset is consumed once");
        let row_y = h - tab_row - TOOLBAR_ROW_HEIGHT / 2.0;
        // The primary is the trailing-most button.
        app.tap(w - EDGE_PAD - 10.0, row_y);
        assert_eq!(app.take_log(), ["action:comment"]);
        assert_eq!(app.nodes(Role::Toolbar).len(), 2, "the top bar and the row");
    }

    #[test]
    fn a_toolbar_menu_expands_above_its_row_and_reports_the_choice() {
        let _guard = slot_test_guard();
        let (_, h) = SIZES[1];
        let mut app = App::new(SIZES[1], Brightness::Dark, WindowInsets::default())
            .with_toolbar(comment_toolbar(), false);
        app.frame();
        let body = app.seen().size.height;
        assert_eq!(body, h - TOP_BAR_HEIGHT - TOOLBAR_ROW_HEIGHT);
        let sort = app
            .nodes(Role::Button)
            .into_iter()
            .find(|(_, node)| node.label() == Some("Sort"))
            .expect("the sort button");
        app.root
            .perform_accessibility_action(&mut (), sort.0, Action::Click);
        app.frame();
        assert!(
            app.take_log().is_empty(),
            "a menu entry reports nothing itself"
        );
        assert_eq!(app.seen().size.height, body - MENU_ROW_HEIGHT);
        assert_eq!(app.nodes(Role::MenuItem).len(), 2);
        let menu_y = h - TOOLBAR_ROW_HEIGHT - MENU_ROW_HEIGHT / 2.0;
        app.tap(2.0 * EDGE_PAD + MENU_ITEM_PAD, menu_y);
        assert_eq!(app.take_log(), ["menu:sort/new"]);
        assert_eq!(app.seen().size.height, body, "a choice collapses the row");
    }

    #[test]
    fn the_toolbar_row_takes_the_bottom_inset_when_it_is_the_lowest_bar() {
        let _guard = slot_test_guard();
        let (_, h) = SIZES[0];
        let mut app = App::new(SIZES[0], Brightness::Light, insets(0.0, 0.0, 34.0))
            .with_toolbar(comment_toolbar(), false);
        let rec = app.frame();
        assert!(rec.glyph_runs >= 3, "the row's labels");
        let seen = app.seen();
        assert_eq!(
            seen.size.height,
            h - TOP_BAR_HEIGHT - TOOLBAR_ROW_HEIGHT - 34.0
        );
        assert_eq!(seen.bottom, 0.0);
    }

    fn tab_row(faces: Vec<TabFace>) -> TabRowWidget {
        let view = TabRowView {
            spec: ChromeSpec {
                tabs: vec![
                    TabItem::new("home", "Home", TabIcon::AppleSymbol("house".into())),
                    TabItem::new("inbox", "Inbox", TabIcon::AppleSymbol("tray".into())),
                ],
                selected: Some(TabId::new("home")),
                ..ChromeSpec::default()
            },
            faces,
            generation: ChromeGeneration(1),
        };
        let mut counter = 0;
        <TabRowView as View<()>>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn face(title: &str, icon: TabIcon, badge: &str, enabled: bool) -> TabFace {
        TabFace {
            title: title.into(),
            icon: Some(icon),
            selected_icon: None,
            badge: Some(badge.into()),
            enabled,
        }
    }

    #[test]
    fn a_tab_cell_draws_its_icon_badge_and_disabled_state() {
        let mut row = tab_row(vec![
            face("Home", TabIcon::AppleSymbol("house".into()), "3", true),
            face(
                "Inbox",
                TabIcon::Bytes(Arc::from(&b"not an image"[..])),
                "",
                false,
            ),
        ]);
        let theme = Theme::neutral();
        let mut text = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text).with_theme(&theme);
        let bc = BoxConstraints::new(Size::new(390.0, 0.0), Size::new(390.0, 200.0));
        let size = row.layout(&mut lctx, &bc);
        assert_eq!(size, Size::new(390.0, TAB_ROW_HEIGHT));
        let mut rec = Rec::default();
        row.paint(
            &mut PaintCtx::new(Point::ZERO, size).with_theme(&theme),
            &mut rec,
        );
        // Two titles, the "3" badge and the house symbol's letter.
        assert_eq!(rec.glyph_runs, 4);
        assert_eq!(rec.images, 0, "undecodable bytes draw nothing");
        let dot = row.cells[1].badge_rect.expect("an empty badge is a dot");
        assert_eq!(dot.size(), Size::new(BADGE_DOT, BADGE_DOT));
        assert!(row.cells[0].badge_rect.unwrap().width() >= BADGE_HEIGHT);
        assert!(!row.targets[1].enabled);
        let down = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: row.targets[1].rect.center(),
            button: PointerButton::Primary,
        });
        let mut state = ();
        let mut ctx = EventCtx::new(&mut state, Point::ZERO, size);
        assert_eq!(row.event(&mut ctx, &down), EventResult::Ignored);
    }

    #[test]
    fn a_row_built_from_real_tab_items_shows_their_faces() {
        let _guard = slot_test_guard();
        let items = [
            TabItem::new("home", "Home", TabIcon::AppleSymbol("house".into())).badge("3"),
            TabItem::new("inbox", "Inbox", TabIcon::AppleSymbol("tray".into()))
                .selected_icon(TabIcon::AppleSymbol("tray.fill".into()))
                .enabled(false),
        ];
        let faces: Vec<TabFace> = items.iter().map(tab_face).collect();
        assert_eq!(faces[0].title, "Home");
        assert_eq!(faces[0].badge.as_deref(), Some("3"));
        assert!(faces[0].enabled);
        assert_eq!(faces[1].title, "Inbox");
        assert_eq!(
            faces[1].selected_icon,
            Some(TabIcon::AppleSymbol("tray.fill".into()))
        );
        assert!(!faces[1].enabled);

        let mut row = tab_row(faces);
        let theme = Theme::neutral();
        let mut text = TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text).with_theme(&theme);
        let bc = BoxConstraints::new(Size::new(390.0, 0.0), Size::new(390.0, 200.0));
        let size = row.layout(&mut lctx, &bc);
        let mut rec = Rec::default();
        row.paint(
            &mut PaintCtx::new(Point::ZERO, size).with_theme(&theme),
            &mut rec,
        );
        // Two titles, the "3" badge and two icon letters.
        assert_eq!(rec.glyph_runs, 5);
        assert!(row.cells[0].badge_rect.is_some());
        assert!(row.cells[1].badge_rect.is_none());
        assert!(!row.targets[1].enabled);
        let down = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Down,
            position: row.targets[1].rect.center(),
            button: PointerButton::Primary,
        });
        let mut state = ();
        let mut ctx = EventCtx::new(&mut state, Point::ZERO, size);
        assert_eq!(row.event(&mut ctx, &down), EventResult::Ignored);
    }

    #[test]
    fn symbol_letters_take_the_first_letter_of_two_parts() {
        assert_eq!(symbol_letters("house"), "H");
        assert_eq!(symbol_letters("square.and.arrow.up"), "SA");
        assert_eq!(symbol_letters(""), "");
    }
}
