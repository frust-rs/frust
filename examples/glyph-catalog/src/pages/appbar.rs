//! AppBar section: six demo blocks — anatomy, scroll collapse, back-nav
//! title crossfade, the overflow menu, selection mode, and the connection
//! banner. The catalog's own root [`AppBar`](frust::glyph::app_bar)
//! is the real integration; this section is the reference showcase, kept
//! visually consistent with it.
//!
//! # Structure
//!
//! `pub fn page(state)` is a launcher: one inline anatomy diagram
//! ([`demo_anatomy`], the "Anatomy" moment, kept inline deliberately)
//! over a [`glyph_list`] of six variation rows. Each row's
//! `on_press` routes through [`open_variation`] to one of six `open_*`
//! functions (`pub` — see their own docs), which `state.nav.push`es a real,
//! full-screen page built by the matching `variation_*` fn: a real
//! [`app_bar`] at top (so inset consumption/scroll-collapse/elevation are
//! exercised for real, not simulated inline)
//! over a [`variation_frame`]-wrapped scrollable body, with a
//! [`back_button`] leading slot popping the *outer* (root) navigator back to
//! this launcher. Page-local demo state is cached in a self-healing
//! `thread_local!` (the `local_sig!` macro, duplicated from
//! `interactions.rs`/`motion.rs`'s macro of the same shape — the existing
//! per-module precedent, not shared across files) exactly as it was when
//! these six moments rendered inline, so a variation's state now simply
//! survives a push→pop→push round-trip instead of a rebuild.
//!
//! # The overflow-menu anchor: a genuine framework gap
//!
//! [`frust::glyph::show_glyph_menu`]'s anchor is a caller-reported
//! window-coordinate `Rect` (`frust::kurbo::Rect`, flat-re-exported by
//! `frust::authoring`; `frust_widgets::glyph::menu`'s module docs describe how an `AppBar`'s
//! trailing icon records its own painted bounds during `paint`) — there is
//! no ancestor-bounds query a widget can make mid-layout, and no facade
//! widget reports a child's painted bounds back to app code. [`AnchorReporter`]
//! is this crate's minimal answer: a hand-rolled `View`/`Widget` pair reached
//! entirely through `frust::authoring`, mirroring
//! `examples/huddle::ui::fill_box::FilledBox`'s documented low-level widget
//! pattern exactly: it paints its child unchanged and, on every
//! paint pass, stashes `ctx.origin()`/`ctx.size()` into a shared
//! `Rc<Cell<Rect>>` the kebab's `on_press` handler reads back when opening
//! the menu.
//!
//! # Timer-driven demos with no `tokio` dependency
//!
//! The connection-banner countdown needs a real interval, but this
//! crate has no direct `tokio` dependency (`interactions.rs`'s module docs:
//! the framework's blessed `tokio::time::sleep` idiom needs one this crate
//! doesn't carry). [`Delay`] is a minimal, dependency-free one-shot timer
//! future (`std::thread::sleep` on a throwaway thread, waking the polling
//! task's `Waker` on completion) `frust::spawn_local` awaits between each
//! countdown tick — the same background-thread-wakes-a-`spawn_local`-task
//! wiring `docs/ARCHITECTURE.md`'s Signal-driven wake documents for a real
//! tokio timer, so the redraw-on-background-wake contract holds with no new
//! dependency at all.

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

// AnchorReporter (see the module docs' "genuine framework gap" section) is a
// hand-rolled `View`/`Widget` pair — reached, like every other widget in this
// file, entirely through the `frust` facade.
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, Point, Rect, SemanticsCtx, Size, Widget,
};
use frust::glyph::{
    BadgeVariant, BannerVariant, MenuEntry, TitleDirection, app_bar, banner_spec, glyph_list,
    glyph_list_item, large_config, menu_item, menu_item_danger, menu_separator, selection_bar,
    show_glyph_menu,
};
use frust::motion::patterns::SharedAxis;
use frust::motion::switcher::pattern_switcher;
use frust::{
    AnyView, Axis, ButtonStyle, Color, CrossAxisAlignment, EdgeInsets, FlexChild, FlexView, Get,
    GetUntracked, Image, ImageFit, ImageSource, NavigatorController, Padding, PopResult, RwSignal,
    ScrollInfo, Set, SizedBox, Theme, View, any, button, checkbox, flexible, inflexible, safe_area,
    scroll_view, spawn_local, text, use_context,
};

use crate::CatalogState;

// ---------------------------------------------------------------------------
// Section chrome (see interactions.rs/motion.rs's identical helpers)
// ---------------------------------------------------------------------------

/// Live-theme accent-text role (`primary`), falling back to the Glyph
/// baseline pre-context — see `navigation.rs`'s `accent()` twin.
fn amber() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .primary
}

/// A muted caption ink — see [`amber`]'s twin.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .on_surface_variant
}

/// `color` with its alpha channel replaced — duplicated from
/// `frust-widgets::glyph::badge`'s crate-private helper of the same shape
/// (unreachable from here), see `interactions.rs`'s identical duplicate.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A 1×1 solid-color [`ImageSource`] — the facade-only way to paint an
/// arbitrary filled rectangle (`interactions.rs`'s documented technique,
/// duplicated locally per that file's own note that this crate has no
/// generic "filled box" widget).
fn solid_source(color: Color) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), to_u8(c[3])],
        1,
        1,
    )
}

/// A demo heading in accent amber.
fn label(s: impl Into<String>) -> AnyView<CatalogState> {
    any(text(s).size(13.0).color(amber()))
}

/// A muted per-demo caption.
fn caption(s: impl Into<String>) -> AnyView<CatalogState> {
    any(text(s).size(11.0).color(muted()))
}

/// A fixed-height vertical spacer between demo blocks.
fn gap(h: f64) -> FlexChild<CatalogState> {
    inflexible(SizedBox(None, Some(h)))
}

/// Wrap a demo's rows in a padded vertical column (one showcase card) —
/// mirrors `interactions.rs`/`motion.rs`'s helper of the same name/shape.
fn block(children: Vec<FlexChild<CatalogState>>) -> FlexChild<CatalogState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

// ---------------------------------------------------------------------------
// Pushed-page frame — shared by every variation (see the module docs' "Structure")
// ---------------------------------------------------------------------------

/// A small leading "‹" icon button popping the *outer* (root) navigator back
/// to the launcher list — every variation page's own back affordance, working
/// via the auto-wired navigator.
/// `nav` is the page's own captured [`NavigatorController`] clone, threaded
/// in by [`open_variation`] at push time (mirrors the dialog demos' push
/// pattern).
fn back_button(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    any(button("\u{2039}", move |_: &mut CatalogState| nav.pop())
        .style(ButtonStyle::Icon)
        .small())
}

/// Wrap a variation page's real `bar` over a scrollable `body` — the pushed
/// full-screen frame every variation shares. `bar` consumes the top window
/// inset itself (`glyph::app_bar`'s own module docs), so `body`'s safe area
/// disables its own top edge (`.top(false)`) to avoid double-padding it,
/// exactly mirroring `crate::lib`'s root shell composition.
fn variation_frame(
    bar: AnyView<CatalogState>,
    body: AnyView<CatalogState>,
) -> AnyView<CatalogState> {
    any(FlexView::new(
        Axis::Vertical,
        vec![inflexible(bar), flexible(1, safe_area(body).top(false))],
    ))
}

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner —
/// duplicated from `interactions.rs`/`motion.rs`'s macro of the same
/// shape/rationale (private, not exported across page modules).
macro_rules! local_sig {
    ($name:ident, $ty:ty, $init:expr) => {
        fn $name() -> RwSignal<$ty> {
            thread_local! {
                static SLOT: std::cell::RefCell<Option<RwSignal<$ty>>> =
                    const { std::cell::RefCell::new(None) };
            }
            SLOT.with(|cell| {
                if let Some(sig) = *cell.borrow()
                    && sig.try_get_untracked().is_some()
                {
                    return sig;
                }
                let sig = RwSignal::new($init);
                *cell.borrow_mut() = Some(sig);
                sig
            })
        }
    };
}

// ---------------------------------------------------------------------------
// AnchorReporter — the overflow-menu anchor (see module docs)
// ---------------------------------------------------------------------------

/// Paints `child` unchanged but records its own window-coordinate rect into
/// `target` on every paint pass — see the [module docs](self)'s "genuine
/// framework gap" section. Mirrors `examples/huddle::ui::fill_box::FilledBox`'s
/// single-child `ChildPod` shape exactly.
struct AnchorReporter<State: 'static> {
    child: AnyView<State>,
    target: Rc<Cell<Rect>>,
}

/// Wrap `child` so its painted bounds land in `target` every paint pass.
fn anchor_reporter<State: 'static, V: View<State>>(
    child: V,
    target: Rc<Cell<Rect>>,
) -> AnchorReporter<State> {
    AnchorReporter {
        child: any(child),
        target,
    }
}

/// The retained widget for an [`AnchorReporter`].
struct AnchorReporterWidget {
    child: ChildPod,
    target: Rc<Cell<Rect>>,
}

/// Build a [`ChildPod`] wrapping an [`AnyView`]'s element (mirrors
/// `frust-widgets::build_child`, re-derived because that helper is
/// crate-private — the same re-derivation `ui::fill_box::FilledBox` uses).
fn build_child<State: 'static>(view: &AnyView<State>, ctx: &mut BuildCtx<'_>) -> ChildPod {
    let element: Box<dyn Widget> = view.build(ctx);
    ChildPod::new(Box::new(element))
}

/// Reconcile the child through its `ChildPod` (mirrors
/// `frust-widgets::rebuild_child`).
fn rebuild_child<State: 'static>(
    prev: &AnyView<State>,
    next: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) -> ChangeFlags {
    let element = pod
        .widget_mut()
        .downcast_mut::<Box<dyn Widget>>()
        .expect("anchor_reporter child element is a boxed AnyView widget");
    next.rebuild(prev, element, ctx)
}

/// Tear the child down (mirrors `frust-widgets::teardown_child`).
fn teardown_child<State: 'static>(
    view: &AnyView<State>,
    pod: &mut ChildPod,
    ctx: &mut BuildCtx<'_>,
) {
    if let Some(element) = pod.widget_mut().downcast_mut::<Box<dyn Widget>>() {
        view.teardown(element, ctx);
    }
}

impl<State: 'static> View<State> for AnchorReporter<State> {
    type Element = AnchorReporterWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AnchorReporterWidget {
        AnchorReporterWidget {
            child: build_child(&self.child, ctx),
            target: self.target.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AnchorReporterWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.target = self.target.clone();
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx) | ChangeFlags::PAINT
    }

    fn teardown(&self, element: &mut AnchorReporterWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AnchorReporterWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.target
            .set(Rect::from_origin_size(ctx.origin(), ctx.size()));
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }
}

/// The kebab's own painted rect, cached in a process-wide `thread_local!`
/// (not reactive — a plain `Rc<Cell<Rect>>`, so it never needs the
/// self-healing dance [`local_sig!`]'s `RwSignal`s do).
fn kebab_anchor() -> Rc<Cell<Rect>> {
    thread_local! {
        static ANCHOR: Rc<Cell<Rect>> = Rc::new(Cell::new(Rect::ZERO));
    }
    ANCHOR.with(|a| a.clone())
}

// ---------------------------------------------------------------------------
// Delay — a dependency-free one-shot timer future (see module docs)
// ---------------------------------------------------------------------------

/// A minimal one-shot timer future built on `std::thread`/`Waker` — see the
/// [module docs](self)'s "Timer-driven demos" section for why this crate
/// hand-rolls one instead of awaiting `tokio::time::sleep`.
struct Delay {
    deadline: Instant,
}

impl Delay {
    fn new(duration: Duration) -> Self {
        Delay {
            deadline: Instant::now() + duration,
        }
    }
}

impl Future for Delay {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let now = Instant::now();
        if now >= self.deadline {
            Poll::Ready(())
        } else {
            let remaining = self.deadline - now;
            let waker = cx.waker().clone();
            std::thread::spawn(move || {
                std::thread::sleep(remaining);
                waker.wake();
            });
            Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// 01 — anatomy
// ---------------------------------------------------------------------------

local_sig!(anatomy_starred_sig, bool, false);

/// A small square filled with `color`, the brand-mark's unit cell.
fn mark_cell(color: Color) -> AnyView<CatalogState> {
    any(SizedBox(Some(8.0), Some(8.0)).child(Image(solid_source(color)).fit(ImageFit::Fill)))
}

/// The design's 2×2 grid brand mark, composed from four small filled
/// squares — a checkerboard of `accent`/a faded `accent` (the anatomy demo's
/// leading slot: a back arrow / brand mark / nothing).
fn brand_mark() -> AnyView<CatalogState> {
    let accent = amber();
    let faded = with_alpha(accent, 0.4);
    any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(mark_cell(accent)),
                    gap(2.0),
                    inflexible(mark_cell(faded)),
                ],
            )),
            gap(2.0),
            inflexible(FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(mark_cell(faded)),
                    gap(2.0),
                    inflexible(mark_cell(accent)),
                ],
            )),
        ],
    ))
}

/// 01 anatomy: a compact bar — brand-mark leading, a title + subtitle, one
/// trailing action (a star toggle, giving the "one trailing action" some
/// minimal interactivity beyond a static screenshot).
fn demo_anatomy() -> FlexChild<CatalogState> {
    let starred_sig = anatomy_starred_sig();
    let starred = starred_sig.get();

    let bar = app_bar::<CatalogState>("dev · session")
        .subtitle("9 panes · running")
        .leading(brand_mark())
        .actions(vec![any(button(
            if starred { "★" } else { "☆" },
            move |_: &mut CatalogState| starred_sig.set(!starred_sig.get_untracked()),
        )
        .style(ButtonStyle::Icon)
        .small())]);

    block(vec![
        inflexible(label("01 Anatomy")),
        inflexible(caption(
            "brand mark leading · title + subtitle · one trailing action",
        )),
        gap(6.0),
        inflexible(bar),
    ])
}

// ---------------------------------------------------------------------------
// 02 — scroll collapse
// ---------------------------------------------------------------------------

local_sig!(collapse_progress_sig, f64, 0.0);
local_sig!(collapse_elevated_sig, bool, false);

/// Scroll offset (logical px) past which [`AppBarView::large`] is fully
/// collapsed — matches `AppBarView::collapse_progress`'s own `min(1,
/// offset/60)` mapping
/// (`frust_widgets::glyph::appbar::AppBarView::collapse_progress`'s docs).
const COLLAPSE_SPAN_PX: f64 = 60.0;
/// Scroll offset (logical px) past which the bar also raises its scrolled
/// elevation — mirrors [`crate::ELEVATION_THRESHOLD_PX`] (the root AppBar's
/// own threshold, kept in sync per this section's "visually consistent"
/// charter — see the [module docs](self)).
const ELEVATED_THRESHOLD_PX: f64 = 4.0;

/// A short "connected + latency" meta row — a lightweight stand-in for the
/// interactions section's fuller connection-heartbeat composition (its own
/// `demo_heartbeat` is private, so a small duplicate here was preferred over
/// editing that file). Shown under the large variant's big title.
fn collapse_meta_row() -> AnyView<CatalogState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(any(
                frust::glyph::badge("connected", BadgeVariant::Success).dot(true)
            )),
            gap(8.0),
            inflexible(text("100.71.31.57:50051 · 42ms").size(11.0).color(muted())),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// A bounded list of filler rows — the scrollable content that drives the
/// bar's collapse via its own `on_scroll`.
///
/// 64 rows, deliberately: at ~31 logical px per row this is ~2000px of
/// content, comfortably taller than any phone viewport (~870 logical px on
/// a 1080×2400 @ 2.75 DPR panel). The original 24 rows (~740px) barely
/// exceeded a tall device's body height, leaving a ~70px scroll budget that
/// read as "scroll is broken" on device and never reached the collapse
/// span — while the 700px-tall headless test viewport made the same filler
/// scroll generously, so every gate stayed green (the M1 device-only
/// mystery, root-caused 2026-07-24).
fn filler_rows() -> AnyView<CatalogState> {
    let rows: Vec<AnyView<CatalogState>> = (1..=64)
        .map(|i| {
            any(Padding(
                EdgeInsets::symmetric(0.0, 8.0),
                text(format!("row {i:02} — pane output line")).size(12.0),
            ))
        })
        .collect();
    any(FlexView::new(
        Axis::Vertical,
        rows.into_iter().map(inflexible).collect(),
    ))
}

/// 02 scroll-collapse large: the real pushed page. A full-height filler
/// `ScrollView` feeds `collapse_progress`/`elevated` from its own `on_scroll`
/// into a `.large(..)` bar with a heartbeat-style meta row and a leading back
/// button — the inline demo's exact bar wiring, now over real inset-consuming
/// chrome instead of a fixed-height `SizedBox` preview.
fn variation_scroll_collapse(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let progress_sig = collapse_progress_sig();
    let elevated_sig = collapse_elevated_sig();
    let progress = progress_sig.get();
    let elevated = elevated_sig.get();

    let bar = app_bar::<CatalogState>("dev · session")
        .large(large_config("dev · session", collapse_meta_row()))
        .leading(back_button(nav))
        .collapse_progress(progress)
        .elevated(elevated);

    let body = any(scroll_view(filler_rows()).on_scroll(
        move |_state: &mut CatalogState, info: ScrollInfo| {
            progress_sig.set((info.offset / COLLAPSE_SPAN_PX).clamp(0.0, 1.0));
            elevated_sig.set(info.offset > ELEVATED_THRESHOLD_PX);
        },
    ));

    variation_frame(any(bar), body)
}

/// Push [`variation_scroll_collapse`] onto the outer navigator — `pub`: the
/// headless push/pop regression test (`tests/smoke.rs`) calls this directly,
/// exactly mirroring how normal app code reaches it (the launcher list's
/// `on_press`, via [`open_variation`]).
pub fn open_scroll_collapse(state: &mut CatalogState) {
    let nav = state.nav.clone();
    state
        .nav
        .push(move || variation_scroll_collapse(nav.clone()));
}

/// Read the scroll-collapse demo's progress signal — the value
/// [`variation_scroll_collapse`]'s `ScrollView::on_scroll` feeds from the live
/// scroll offset (`(offset / COLLAPSE_SPAN_PX).clamp(0.0, 1.0)`). The headless
/// scroll-repro regression test (`tests/smoke.rs`) reads it back after
/// dispatching a real drag through a `RenderRoot` to prove the pushed page's
/// `ScrollView` offset actually moved (a nonzero progress means `on_scroll`
/// fired, i.e. the drag reached the scroll widget instead of dying in the
/// navigator/routing chain — the exact 0px-movement defect this regression
/// test guards against).
/// `pub`-for-test only, mirroring [`open_scroll_collapse`]'s own seam rationale
/// and `interactions::start_heartbeat_for_test`'s precedent; never called from
/// `build`.
pub fn scroll_collapse_progress_for_test() -> f64 {
    collapse_progress_sig().get_untracked()
}

/// Reset the scroll-collapse demo's signals to their at-rest defaults — the
/// deterministic baseline the scroll-repro test establishes *after* settling a
/// push transition and *before* dispatching its drag, so the subsequent
/// [`scroll_collapse_progress_for_test`] assertion reads only this drag's
/// contribution (the signals are thread-local and self-healing, so a prior
/// test on the same worker thread could otherwise leave a stale nonzero
/// value). `pub`-for-test only; never called from `build`.
pub fn reset_scroll_collapse_for_test() {
    collapse_progress_sig().set(0.0);
    collapse_elevated_sig().set(false);
}

// ---------------------------------------------------------------------------
// 03 — back-nav title crossfade
// ---------------------------------------------------------------------------

local_sig!(backnav_page_sig, usize, 0);
local_sig!(backnav_back_sig, bool, false);

const BACKNAV_TITLES: [&str; 2] = ["Sessions", "dev · session"];

/// The fake screen underneath the bar for the current `page` index — a
/// two-button demo swapping title and sliding a fake screen underneath.
fn backnav_screen(page: usize) -> AnyView<CatalogState> {
    if page == 0 {
        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(text("3 active sessions").size(12.5)),
                gap(4.0),
                inflexible(caption("tap \u{2039} Open session to push a detail screen")),
            ],
        ))
    } else {
        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(text("dev — 9 panes · 4 tabs · running").size(12.5)),
                gap(4.0),
                inflexible(caption("tap \u{2039} Close session to pop back")),
            ],
        ))
    }
}

/// 03 title crossfade / back-nav: the real pushed page. Two buttons swap the
/// bar's title (with a direction hint) and slide a fake screen underneath via
/// [`frust::motion::patterns::SharedAxis::X`], reversed on the "back" leg —
/// the inline demo's exact wiring, plus a leading [`back_button`] popping the
/// *outer* navigator (distinct from the in-page "Close session" leg, which
/// only crossfades the title/screen, never leaves this page).
fn variation_backnav(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let page_sig = backnav_page_sig();
    let back_sig = backnav_back_sig();
    let page = page_sig.get();
    let back = back_sig.get();

    let bar = app_bar::<CatalogState>(BACKNAV_TITLES[page])
        .leading(back_button(nav))
        .title_direction(if back {
            TitleDirection::Back
        } else {
            TitleDirection::Forward
        });

    let screen = pattern_switcher(page, SharedAxis::X, backnav_screen(page)).reverse(back);

    let buttons = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(
                button("Open session \u{203a}", move |_: &mut CatalogState| {
                    back_sig.set(false);
                    page_sig.set(1);
                })
                .style(ButtonStyle::Secondary)
                .small(),
            ),
            gap(8.0),
            inflexible(
                button("\u{2039} Close session", move |_: &mut CatalogState| {
                    back_sig.set(true);
                    page_sig.set(0);
                })
                .style(ButtonStyle::Ghost)
                .small(),
            ),
        ],
    );

    let body = any(scroll_view(any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(caption(
                "the title crossfades in the tapped direction; the screen below slides the same way",
            )),
            gap(6.0),
            inflexible(screen),
            gap(12.0),
            inflexible(buttons),
            gap(12.0),
            inflexible(filler_rows()),
        ],
    ))));

    variation_frame(any(bar), body)
}

/// Push [`variation_backnav`] onto the outer navigator — see
/// [`open_scroll_collapse`]'s doc for the `pub`-as-test-seam rationale shared
/// by every `open_*` fn in this module.
pub fn open_backnav(state: &mut CatalogState) {
    let nav = state.nav.clone();
    state.nav.push(move || variation_backnav(nav.clone()));
}

// ---------------------------------------------------------------------------
// 04 — overflow menu
// ---------------------------------------------------------------------------

local_sig!(overflow_caption_sig, String, String::new());

/// The kebab's overflow entries: a Danger "Kill session" + a separator.
fn overflow_menu_entries() -> Vec<MenuEntry> {
    vec![
        menu_item("Rename session"),
        menu_item("Duplicate session"),
        menu_item("Export layout\u{2026}"),
        menu_separator(),
        menu_item_danger("Kill session"),
    ]
}

/// 04 overflow menu: the real pushed page. A kebab trailing icon opens
/// [`frust::glyph::show_glyph_menu`], anchored at the kebab's own painted
/// rect via [`AnchorReporter`] (see the [module docs](self)) — the inline
/// demo's exact wiring, plus a leading [`back_button`] and scrollable filler
/// content underneath.
fn variation_overflow(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let caption_sig = overflow_caption_sig();
    let result_caption = caption_sig.get();
    let target = kebab_anchor();

    let kebab_target = target.clone();
    let kebab = any(anchor_reporter(
        button("\u{22ee}", move |state: &mut CatalogState| {
            let anchor = kebab_target.get();
            show_glyph_menu(
                &state.nav,
                anchor,
                overflow_menu_entries,
                move |_: &mut CatalogState, result: PopResult| {
                    let entries = overflow_menu_entries();
                    let text = match result.take::<usize>() {
                        Some(i) => match entries.get(i) {
                            Some(MenuEntry::Item(item)) => format!("Overflow: {}", item.label),
                            _ => "Overflow: dismissed".to_string(),
                        },
                        None => "Overflow: dismissed".to_string(),
                    };
                    caption_sig.set(text);
                },
            );
        })
        .style(ButtonStyle::Icon)
        .small(),
        target,
    ));

    let bar = app_bar::<CatalogState>("dev · session")
        .leading(back_button(nav))
        .actions(vec![kebab]);

    let mut top_children = vec![inflexible(caption(
        "the kebab opens a menu anchored at its own corner; scrim tap/back/Escape dismiss it",
    ))];
    if !result_caption.is_empty() {
        top_children.push(gap(6.0));
        top_children.push(inflexible(caption(result_caption)));
    }
    top_children.push(gap(6.0));

    let body = any(scroll_view(any(FlexView::new(
        Axis::Vertical,
        vec![block(top_children), inflexible(filler_rows())],
    ))));

    variation_frame(any(bar), body)
}

/// Push [`variation_overflow`] onto the outer navigator — see
/// [`open_scroll_collapse`]'s doc for the `pub`-as-test-seam rationale shared
/// by every `open_*` fn in this module.
pub fn open_overflow(state: &mut CatalogState) {
    let nav = state.nav.clone();
    state.nav.push(move || variation_overflow(nav.clone()));
}

// ---------------------------------------------------------------------------
// 05 — selection mode
// ---------------------------------------------------------------------------

local_sig!(selection_checked_sig, [bool; 3], [false; 3]);

const SELECTION_TOKENS: [&str; 3] = [
    "token-a1b2 — CI deploy key",
    "token-c3d4 — Local dev key",
    "token-e5f6 — Backup relay key",
];

/// One checkbox row driving [`selection_checked_sig`]'s `i`-th slot.
fn token_row(i: usize, checked: bool, sig: RwSignal<[bool; 3]>) -> FlexChild<CatalogState> {
    inflexible(checkbox(
        checked,
        SELECTION_TOKENS[i],
        move |_: &mut CatalogState, v: bool| {
            let mut next = sig.get_untracked();
            next[i] = v;
            sig.set(next);
        },
    ))
}

/// 05 selection mode: the real pushed page. Three token rows with checkboxes
/// drive `AppBarView::selection` (the widget-owned × close clears every
/// checkbox) — the inline demo's exact wiring, plus a leading [`back_button`]
/// and scrollable filler content underneath the token rows.
fn variation_selection(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let sig = selection_checked_sig();
    let checked = sig.get();
    let count = checked.iter().filter(|c| **c).count();

    let selection = (count > 0).then(|| {
        selection_bar(count, move |_: &mut CatalogState| {
            sig.set([false; 3]);
        })
        .actions(vec![any(button("Delete", move |_: &mut CatalogState| {
            sig.set([false; 3]);
        })
        .style(ButtonStyle::Danger)
        .small())])
    });

    let bar = app_bar::<CatalogState>("Sessions")
        .leading(back_button(nav))
        .selection(selection);

    let mut children = vec![inflexible(caption(
        "check a token to enter selection mode; \u{d7} clears",
    ))];
    children.push(gap(6.0));
    for (i, c) in checked.iter().enumerate() {
        children.push(token_row(i, *c, sig));
    }
    children.push(gap(12.0));
    children.push(inflexible(filler_rows()));

    let body = any(scroll_view(any(FlexView::new(Axis::Vertical, children))));

    variation_frame(any(bar), body)
}

/// Push [`variation_selection`] onto the outer navigator — see
/// [`open_scroll_collapse`]'s doc for the `pub`-as-test-seam rationale shared
/// by every `open_*` fn in this module.
pub fn open_selection(state: &mut CatalogState) {
    let nav = state.nav.clone();
    state.nav.push(move || variation_selection(nav.clone()));
}

// ---------------------------------------------------------------------------
// 06 — connection banner
// ---------------------------------------------------------------------------

/// The banner sequence's phase — driven entirely off its own [`Delay`]
/// timers, never a per-frame wall-clock read (unlike `interactions.rs`'s
/// clock-driven demos, this one only needs to update at each countdown
/// tick, not every frame).
#[derive(Clone, Copy, PartialEq)]
enum BannerPhase {
    Idle,
    Disconnected { remaining: u32 },
    Reconnected,
}

local_sig!(banner_phase_sig, BannerPhase, BannerPhase::Idle);
// A generation counter guarding overlapping sequences: a second "simulate
// disconnect" tap bumps this, and the stale, still-running sequence's own
// ticks no-op once they see their captured generation is no longer current
// (mirrors `interactions.rs`'s `boot_replay_sig` replay-guard shape). A plain
// `//` comment (not `///`) — a doc comment directly above a macro invocation
// warns under `unused_doc_comments` (rustdoc can't attach it to the
// generated `fn`).
local_sig!(banner_gen_sig, u64, 0);

/// Countdown length, seconds — long enough to watch
/// the warning tick down, short enough to stay a snappy demo.
const BANNER_DISCONNECT_SECS: u32 = 3;
/// Auto-hide delay after the "Reconnected" flash (~1.2s).
const BANNER_AUTO_HIDE: Duration = Duration::from_millis(1200);

/// Kick off (or restart) the disconnect → countdown → reconnected →
/// auto-hide sequence, entirely off [`Delay`] timers awaited inside a
/// [`frust::spawn_local`] task — see the [module docs](self)'s "Timer-driven
/// demos" section.
fn start_disconnect_sequence() {
    let phase_sig = banner_phase_sig();
    let gen_sig = banner_gen_sig();
    let my_gen = gen_sig.get_untracked() + 1;
    gen_sig.set(my_gen);
    phase_sig.set(BannerPhase::Disconnected {
        remaining: BANNER_DISCONNECT_SECS,
    });

    spawn_local(async move {
        for step in 1..=BANNER_DISCONNECT_SECS {
            Delay::new(Duration::from_secs(1)).await;
            if gen_sig.get_untracked() != my_gen {
                return;
            }
            let remaining = BANNER_DISCONNECT_SECS - step;
            if remaining > 0 {
                phase_sig.set(BannerPhase::Disconnected { remaining });
            } else {
                phase_sig.set(BannerPhase::Reconnected);
            }
        }
        Delay::new(BANNER_AUTO_HIDE).await;
        if gen_sig.get_untracked() == my_gen {
            phase_sig.set(BannerPhase::Idle);
        }
    });
}

/// 06 connection banner: the real pushed page. "Simulate disconnect" opens a
/// warning banner with a countdown, flips to a success "reconnected" flash,
/// then auto-hides — the inline demo's exact wiring, plus a leading
/// [`back_button`] and scrollable filler content underneath.
fn variation_banner(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let phase = banner_phase_sig().get();

    let banner = match phase {
        BannerPhase::Idle => None,
        BannerPhase::Disconnected { remaining } => Some(banner_spec(
            format!("Connection lost \u{2014} retrying in {remaining}s\u{2026}"),
            BannerVariant::Warning,
        )),
        BannerPhase::Reconnected => Some(banner_spec("Reconnected", BannerVariant::Success)),
    };

    let bar = app_bar::<CatalogState>("dev · session")
        .leading(back_button(nav))
        .banner(banner);

    let body = any(scroll_view(any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(caption(
                "simulates a dropped connection: a countdown, then a success flash, then auto-hide",
            )),
            gap(6.0),
            inflexible(
                button("Simulate disconnect", |_: &mut CatalogState| {
                    start_disconnect_sequence();
                })
                .small(),
            ),
            gap(12.0),
            inflexible(filler_rows()),
        ],
    ))));

    variation_frame(any(bar), body)
}

/// Push [`variation_banner`] onto the outer navigator — see
/// [`open_scroll_collapse`]'s doc for the `pub`-as-test-seam rationale shared
/// by every `open_*` fn in this module.
pub fn open_banner(state: &mut CatalogState) {
    let nav = state.nav.clone();
    state.nav.push(move || variation_banner(nav.clone()));
}

// ---------------------------------------------------------------------------
// 00 — compact + elevate-on-scroll (an addition beyond the six original
// demo moments; every other variation mirrors an existing design-reference
// demo, but "a plain compact bar raising its shadow on scroll" had none of
// its own since the inline anatomy demo never scrolled)
// ---------------------------------------------------------------------------

local_sig!(compact_elevated_sig, bool, false);

/// Compact + elevate-on-scroll: a plain compact bar (no large/selection/
/// banner face) that raises `elevated` off the filler `ScrollView`'s own
/// `on_scroll` — the root catalog `AppBar`'s own `.elevated(...)` contract
/// (`crate::catalog_app_bar`), demonstrated in isolation.
fn variation_compact_elevate(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let elevated_sig = compact_elevated_sig();
    let elevated = elevated_sig.get();

    let bar = app_bar::<CatalogState>("dev · session")
        .subtitle("compact + elevate-on-scroll")
        .leading(back_button(nav))
        .elevated(elevated);

    let body = any(scroll_view(filler_rows()).on_scroll(
        move |_state: &mut CatalogState, info: ScrollInfo| {
            elevated_sig.set(info.offset > ELEVATED_THRESHOLD_PX);
        },
    ));

    variation_frame(any(bar), body)
}

/// Push [`variation_compact_elevate`] onto the outer navigator — see
/// [`open_scroll_collapse`]'s doc for the `pub`-as-test-seam rationale shared
/// by every `open_*` fn in this module.
pub fn open_compact_elevate(state: &mut CatalogState) {
    let nav = state.nav.clone();
    state
        .nav
        .push(move || variation_compact_elevate(nav.clone()));
}

// ---------------------------------------------------------------------------
// Launcher list — the section page itself
// ---------------------------------------------------------------------------

/// The six variation rows, in launcher order: `(glyph, title, sub)`. Index
/// order matches [`open_variation`]'s dispatch — keep them in lockstep.
const VARIATIONS: [(&str, &str, &str); 6] = [
    (
        "01",
        "Compact + elevate-on-scroll",
        "raises its shadow past the scroll threshold",
    ),
    (
        "02",
        "Scroll-collapse large",
        "the big title collapses into the compact bar",
    ),
    ("03", "Overflow menu", "a kebab opens an anchored menu"),
    (
        "04",
        "Selection mode",
        "checkbox rows swap in a selection bar",
    ),
    (
        "05",
        "Connection banner",
        "a dropped-connection countdown, then a reconnect flash",
    ),
    (
        "06",
        "Title crossfade / back-nav",
        "the title crossfades in the tapped direction",
    ),
];

/// Route a launcher-row press (its index into [`VARIATIONS`]) to the matching
/// `open_*` fn — the [`glyph_list`]'s `on_press` callback.
fn open_variation(state: &mut CatalogState, index: usize) {
    match index {
        0 => open_compact_elevate(state),
        1 => open_scroll_collapse(state),
        2 => open_overflow(state),
        3 => open_selection(state),
        4 => open_banner(state),
        5 => open_backnav(state),
        _ => {}
    }
}

/// See the page-fn contract in [`crate::pages`]. A launcher: the inline
/// anatomy diagram (deliberately kept inline rather than pushed)
/// over a [`glyph_list`] of the six variation pages (see the [module
/// docs](self)'s "Structure" section) — each row pushes a real full-screen
/// page via [`open_variation`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    let items = VARIATIONS
        .iter()
        .map(|(glyph, title, sub)| glyph_list_item(*glyph, *title).sub(*sub).chevron(true))
        .collect();

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                demo_anatomy(),
                gap(12.0),
                inflexible(label("Variations")),
                inflexible(caption(
                    "each opens as a full-screen page with a real app bar — inset consumption, \
                     scroll collapse, and back all live",
                )),
                gap(6.0),
                inflexible(any(glyph_list(items)
                    .on_press(|state: &mut CatalogState, i| open_variation(state, i)))),
            ],
        ),
    ))
}
