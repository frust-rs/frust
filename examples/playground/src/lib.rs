//! Playground — the framework's plugin, native-capability, and general
//! testing showcase. It is deliberately **not** a design-system catalog
//! (`examples/glyph-catalog` owns that) and **not** a benchmark
//! (`benchmarks/` owns that): every section here exists to exercise a real
//! OS-facing capability — platform-view embedding, the camera plugin, native
//! platform controls, window-shape responsiveness, driving a real terminal
//! emulator, and the soft keyboard's IME seam — on a real device.
//!
//! This module is only the **shell**: the app state, the root [`navigator`]
//! (whose home page is a Material [`app_bar`](frust_material::app_bar) — a plug brand
//! mark, the "playground" title, and the brightness/reduce-motion/animations
//! toggles folded into its trailing actions — over a
//! [`pattern_switcher`](frust::motion::switcher::pattern_switcher) hosting one
//! of eleven section pages in a [`scroll_view`], the whole column inside one
//! [`safe_area`] and under a toast overlay), and the [`frust::app!`] entry
//! binding all three platforms. The section pages themselves live in
//! [`pages`] — each a `page(&PlaygroundState) -> AnyView<PlaygroundState>`
//! (see `pages/mod.rs` for the page-fn contract).
//!
//! # Responsive navigation
//!
//! [`pages::SECTION_LABELS`] carries eleven destinations now — too many for a
//! single bottom nav bar to divide evenly without its labels wrapping (see
//! that constant's own doc). [`home_page`] picks between two shapes, the same
//! `use_context::<WindowMetrics>()` / named-breakpoint idiom
//! `pages::responsive` establishes (a missing context — the headless test
//! harness, or a shell that has not yet published one — falls back to the
//! narrow shape, same as that page):
//!
//! - **Narrow** (< [`NAV_WIDE_BREAKPOINT_PX`]): [`bottom_nav_bar`] shows the
//!   first [`pages::PRIMARY_NAV_COUNT`] sections plus a trailing "More"
//!   destination; pressing it opens [`more_overlay`], a full-section list
//!   overlay (dismissed by picking a section, or its own "Close" button).
//! - **Wide** (\u{2265} [`NAV_WIDE_BREAKPOINT_PX`]): [`side_rail`] replaces the
//!   bottom bar with an in-layout column listing every section, beside the
//!   page body (the `frust_material::navigation_rail` mounting contract's
//!   "Standard" shape) — nothing is ever behind an overflow menu at this
//!   width.
//!
//! [`bottom_nav_bar`] still rides the existing
//! [`navigation_bar`](frust_material::navigation_bar)/[`nav_item`](frust_material::nav_item)
//! chrome (neither needs an icon, so growing to 11 labels costs nothing
//! there); [`side_rail`] and [`more_overlay`] are hand-rolled from baseline
//! [`button`]s instead of `frust_material::navigation_rail`/
//! `navigation_drawer` — those destinations carry a *mandatory* icon per
//! entry, and playground names no icon set for its eleven sections (the
//! module docs' design-neutral charter: "Nothing in this app demonstrates a
//! design language"). [`pages::SECTION_LABELS`] stays the single source of
//! truth every shape reads from.
//!
//! Back-dismiss (a pushed overlay/page, then the root navigator) works with
//! **zero** app code — `frust::navigator` (imported below) auto-wires
//! Android/gesture back handling for the shared [`NavigatorController`].
//!
//! Deep links via `frustplay://section/<label>` route to a section through the
//! facade's [`deep_links()`](frust::deep_links) signal, so an external launcher
//! or smoke test can navigate to a section without UI interaction.
//!
//! # Mode B background
//!
//! The generated Android/iOS glue turns `FRUST_TRANSLUCENT_SURFACE`/
//! `translucentSurface` ON (playground is the framework's Mode B testbed —
//! see `pages::platform_views`'s module docs) — a process-wide, compile-time
//! choice, not one scoped to that one section. Under Mode B the frust
//! surface's own base clear turns alpha-0 (`frust-shell-*`'s per-frame
//! `base_color` swap), so **every** pixel no widget explicitly paints becomes
//! a window straight through to whatever sits behind the surface — not just
//! `platform_views`'s own deliberate slot. [`home_page`]'s root [`Stack`]
//! therefore paints an explicit, surface-colored [`AppBackground`] as its
//! bottom-most layer, restoring every other section's opaque look; the
//! `platform_views` section's own slot is the one deliberate hole punched
//! through it.
//!
//! Run it with `cargo run` (desktop preview) or `frust run` (Android/iOS).

pub mod pages;

// The compile-time-loaded Fluent message catalog for the "i18n" section
// (`pages/i18n.rs`) — one `locales!` invocation per module (the macro's own
// contract), so this crate root is the app's only one. Expands, right here,
// to `pub fn locale_set()`, `pub fn engine()`, and `pub mod keys` — reached
// below as `locale_set()`/`crate::keys::…`. `pages/i18n.rs` reaches the
// `keys` module the same way (`crate::keys::…`); `engine()` itself is
// **not** used — it builds an ICU-function-free `Engine`, and the i18n
// page's FTL messages need `NUMBER`/`DATETIME` (see [`setup_i18n`]).
frust_i18n::locales!("locales");

use std::cell::Cell;
use std::rc::Rc;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, Widget,
};

use frust::motion::patterns::SharedAxis;
use frust::motion::switcher::pattern_switcher;
use frust::{
    Align, Alignment, AnyView, Axis, Brightness, ButtonStyle, Color, Component, DeepLink,
    EdgeInsets, FlexView, Get, GetUntracked, MotionScheme, NavigatorController, Padding,
    PageTransition, PanZoomController, PanZoomTransform, RwSignal, Set, SizedBox, Stack, Theme,
    TransitionSpec, Update, View, WindowMetrics, any, button, container, deep_links, flexible,
    icon, icons, inflexible, navigator, safe_area, scroll_view, set_app_theme, text, use_context,
};
use frust_material::{app_bar, nav_item, navigation_bar};

use pages::{PRIMARY_NAV_COUNT, SECTION_LABELS};

/// The width breakpoint past which [`home_page`] shows [`side_rail`] instead
/// of [`bottom_nav_bar`] — the same M3 compact/medium window-size-class
/// cutoff `pages::responsive`'s own `WIDE_BREAKPOINT_PX` uses (one constant
/// per page/shell that needs it, not a shared breakpoint API — see that
/// page's own doc comment).
const NAV_WIDE_BREAKPOINT_PX: f64 = 600.0;

// ---------------------------------------------------------------------------
// Deep-link routing — pure parse/plan functions plus the small root-mounted
// `deep_link_router` component that applies them (see module docs' third
// paragraph and the [`deep_link_router`] doc comment below).
// ---------------------------------------------------------------------------

/// The scheme+host prefix every routed deep link starts with; anything else
/// (a different scheme, a different host) is not a section deep link and is
/// ignored — see [`parse_section_deep_link`].
const SECTION_DEEP_LINK_PREFIX: &str = "frustplay://section/";

/// Extract the `<label>` segment from a `frustplay://section/<label>[/...]`
/// URL. Tolerates a trailing slash (and any further path/query/fragment,
/// unchanged from the original inline parser) by taking everything up to the
/// first `/`, `?`, or `#`. Returns `None` for a non-matching scheme/host or an
/// empty label — this crate does not percent-decode the label (the original
/// inline parser did not either, and `SECTION_LABELS` are all plain ASCII
/// words that never need it).
fn parse_section_deep_link(url: &str) -> Option<&str> {
    let rest = url.strip_prefix(SECTION_DEEP_LINK_PREFIX)?;
    let label = rest.split(['/', '?', '#']).next().unwrap_or("");
    if label.is_empty() { None } else { Some(label) }
}

/// The effect a delivered [`DeepLink`] resolves to, as decided by
/// [`plan_deep_link`] — applied by [`deep_link_router`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeepLinkAction {
    /// Navigate to `index` (a valid [`pages::section_index_for`] result);
    /// `auto_run_db_smoke` is true exactly when `index` is the DB section, so
    /// the DB page auto-runs its smoke test once on arrival.
    GoToSection {
        index: usize,
        auto_run_db_smoke: bool,
    },
    /// The link named a `frustplay://section/<label>` URL, but `<label>`
    /// matched no [`pages::SECTION_LABELS`] entry — carries the raw label for
    /// the toast/log message.
    UnknownLabel(String),
}

/// Decide what (if anything) a delivered `link` should do, given the
/// sequence of the last delivery already applied. Dedupes on
/// [`DeepLink::sequence`] — a monotonic per-delivery counter — rather than
/// comparing URL text, so two deliveries of an *identical* URL are still two
/// separate applications, while a rebuild re-observing the same delivery is a
/// no-op. Returns `None` when the delivery was already applied, or when the
/// URL is not a `frustplay://section/<label>` link at all (silently ignored,
/// matching the original inline parser: a link this app does not route is
/// not this router's business).
fn plan_deep_link(link: &DeepLink, last_applied: Option<u64>) -> Option<DeepLinkAction> {
    if last_applied == Some(link.sequence) {
        return None;
    }
    let label = parse_section_deep_link(&link.url)?;
    match pages::section_index_for(label) {
        Some(index) => {
            let db_index = pages::section_index_for("db").unwrap_or(usize::MAX);
            Some(DeepLinkAction::GoToSection {
                index,
                auto_run_db_smoke: index == db_index,
            })
        }
        None => Some(DeepLinkAction::UnknownLabel(label.to_string())),
    }
}

/// A small root-mounted component, sited beside [`toast_overlay`] in
/// [`home_page`]'s layer stack, that owns the deep-link seam end to end: the
/// single TRACKED read of `frust::deep_links().latest.get()` (the same
/// idiom `pages/url_launcher.rs`'s page uses — `frust-reactive` has no effect
/// API, so a `TrackedScope` re-running this builder on change *is* the
/// reactive seam), planning via [`plan_deep_link`], applying the result to
/// `state`, and recording the delivery's sequence in
/// [`PlaygroundState::last_applied_sequence`] so a later rebuild that
/// re-observes the same delivery is a no-op. Renders nothing (a zero-size
/// [`SizedBox`]) — it exists purely for its side effect on `state`.
fn deep_link_router(state: &PlaygroundState) -> AnyView<PlaygroundState> {
    if let Some(link) = deep_links().latest.get() {
        let last_applied = state.last_applied_sequence.get();
        if let Some(action) = plan_deep_link(&link, last_applied) {
            match action {
                DeepLinkAction::GoToSection {
                    index,
                    auto_run_db_smoke,
                } => {
                    state.section.set(index);
                    state.reverse.set(false);
                    if auto_run_db_smoke {
                        state.auto_run_db_smoke.set(true);
                    }
                    log::info!(
                        "playground deep-link: section {} -> {}",
                        SECTION_LABELS[index],
                        index
                    );
                }
                DeepLinkAction::UnknownLabel(label) => {
                    state.toasts.update(|queue| {
                        queue.push(format!("Unknown section '{label}' in deep link"))
                    });
                    log::warn!("playground deep-link: section {} unknown", label);
                }
            }
        }
        state.last_applied_sequence.set(Some(link.sequence));
    }
    any(SizedBox(Some(0.0), Some(0.0)))
}

// ---------------------------------------------------------------------------
// AppBackground — the Mode B root background layer (see the module docs'
// "Mode B background" section)
// ---------------------------------------------------------------------------

/// A full-bleed background fill — a hand-rolled `View`/`Widget` pair reached
/// entirely through `frust::authoring` (`examples/huddle::ui::fill_box::FillBox`
/// is the identical pattern, built the same way). Needed because no facade widget
/// paints an unconditional "fill available space" rect: `Image`/`SizedBox`
/// only tighten a child when the INCOMING constraint is already tight, and
/// [`Stack`] hands every child a LOOSE (min-zero) constraint, so a naive
/// background child would collapse to zero size. Declaring a large
/// (larger-than-any-real-viewport) intrinsic size and letting
/// [`BoxConstraints::constrain`] clamp it into whatever the `Stack` hands
/// down is the trick — see [`AppBackgroundWidget::layout`].
struct AppBackground(Color);

/// The retained widget for an [`AppBackground`].
struct AppBackgroundWidget(Color);

/// Declared larger than any real viewport (logical px) so
/// [`BoxConstraints::constrain`] always clamps it down to the incoming max —
/// see [`AppBackground`]'s doc comment.
const BACKGROUND_INTRINSIC: f64 = 1.0e7;

impl<State: 'static> View<State> for AppBackground {
    type Element = AppBackgroundWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> AppBackgroundWidget {
        AppBackgroundWidget(self.0)
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut AppBackgroundWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.0 = self.0;
        ChangeFlags::PAINT
    }
}

impl Widget for AppBackgroundWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(BACKGROUND_INTRINSIC, BACKGROUND_INTRINSIC))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        scene.fill_rect(ctx.origin(), ctx.size(), self.0);
    }
}

/// The app's whole reactive surface, bundled so it can be cloned into the
/// navigator's home-page builder (which takes no state argument) and passed by
/// reference to every section page. Every field is `Copy`/`Clone` and
/// `'static`, so [`PlaygroundState`] itself is `Clone` — the handle the shell
/// hands to [`pages::current`] and to overlay pushes.
///
/// All fields are reactive so a write from an event handler (a nav-bar tap, an
/// app-bar toggle, a page's toast trigger) wakes the shell and re-runs the
/// home-page builder.
#[derive(Clone)]
pub struct PlaygroundState {
    /// Light/Dark, driven by the app-bar brightness toggle. Rebuilds the theme
    /// via [`apply_theme`] and `set_app_theme`.
    pub brightness: RwSignal<Brightness>,
    /// The OS/accessibility reduced-motion flag, driven by the app-bar toggle;
    /// mapped into `MotionScheme::reduce_motion` by [`apply_theme`] so every
    /// transition pattern collapses to a short linear crossfade.
    pub reduce_motion: RwSignal<bool>,
    /// The selected section index (0..4); written by the bottom navigation bar.
    pub section: RwSignal<usize>,
    /// Whether the section [`pattern_switcher`] plays its next switch in
    /// reverse (a "back" move), computed from the nav-bar tap index delta.
    pub reverse: RwSignal<bool>,
    /// The FIFO toast queue rendered by [`toast_overlay`]; a page pushes a
    /// message by appending to it.
    pub toasts: RwSignal<Vec<String>>,
    /// The shared navigator controller, so overlay pages (dialogs, sheets) can
    /// push onto the same root stack this shell mounts.
    pub nav: NavigatorController<PlaygroundState>,
    /// The app-bar animations on/off toggle (default on). OFF means "every demo
    /// renders its settled state — no frame requests anywhere": the toggle
    /// is **app-forced reduce motion + demo stop**, not a parallel
    /// mechanism — [`apply_theme`] ORs it into the same
    /// `MotionScheme::reduce_motion` push the app-bar reduce-motion toggle
    /// drives, so every convention-following widget collapses for free.
    pub animations_enabled: RwSignal<bool>,
    /// The [`DeepLink::sequence`] of the last delivery [`deep_link_router`]
    /// applied, guarding against re-applying the same delivery on an
    /// unrelated rebuild. `None` means no delivery has been applied yet.
    /// Compared against `sequence` rather than the URL text so two
    /// deliveries of an identical URL are still two applications (see
    /// [`plan_deep_link`]). A plain `Cell` (not `Rc`-wrapped): only
    /// [`deep_link_router`] ever touches it, and it always runs against the
    /// same `state` [`home_page`] was itself called with — never the
    /// second-level clone made for a section page (see `auto_run_db_smoke`'s
    /// doc comment just below for the case where that distinction matters).
    pub last_applied_sequence: Cell<Option<u64>>,
    /// Flag set by [`deep_link_router`] on a deep-link arrival targeting the
    /// DB section; consumed once by the DB page to auto-run its smoke test.
    /// Initialized false; set true by the router, cleared by the page
    /// function after spawning (debug builds only — see
    /// `pages/database.rs::page`).
    ///
    /// `Rc<Cell<bool>>`, not a bare `Cell`: [`PlaygroundState`] is cloned by
    /// value (`#[derive(Clone)]` above), and `home_page` clones it *again*
    /// (`let handles = state.clone();`) before handing that second clone to
    /// `pages::current`/`database::page` — a bare `Cell<bool>` would clone
    /// its *current value*, not its identity, so a write from
    /// [`deep_link_router`] (using `home_page`'s own `state`) would be
    /// invisible to `database::page` (reading the second-level clone). The
    /// other reactive fields above dodge this because `RwSignal` is `Copy`
    /// over an opaque signal id, not a value — cloning it clones a
    /// reference, not the referent. `Rc` reproduces exactly that sharing for
    /// a value nothing needs to *track*, without paying for an unused
    /// signal subscription.
    pub auto_run_db_smoke: Rc<Cell<bool>>,
    /// Whether the narrow-width bottom nav bar's "More" overlay
    /// ([`more_overlay`]) is open; written by [`bottom_nav_bar`]'s "More"
    /// press and by every destination/"Close" press inside the overlay
    /// itself. Shell-level nav state, not a page's own — lives beside
    /// [`PlaygroundState::section`] for the same reason.
    pub more_open: RwSignal<bool>,
    /// The "Graph" section's selected node index (`pages::graph_canvas`), or
    /// `None` before any node has been tapped. One of the two minimal
    /// additions that section's own state needed — see
    /// [`PlaygroundState::graph_transform`]'s doc for the other.
    pub graph_selected: RwSignal<Option<usize>>,
    /// The "Graph" section's live pan/zoom transform, published by its
    /// `PanZoomView::on_transform` callback and read back by the page's own
    /// scale/offset readout — the second of the two minimal state additions
    /// that section needed (see [`PlaygroundState::graph_selected`]).
    pub graph_transform: RwSignal<PanZoomTransform>,
    /// The "Graph" section's `PanZoomController` handle, so its "Fit" button
    /// (built fresh every rebuild, like every other page) still drives the
    /// *same* attached controller every time — the identical
    /// clone-shares-one-`Rc` shape [`PlaygroundState::nav`] already uses for
    /// `NavigatorController`, not a reactive signal (a controller command is
    /// recorded, not tracked — see `frust::PanZoomController`'s own doc).
    pub graph_controller: PanZoomController,
}

impl PlaygroundState {
    /// Construct the initial state: section 0, dark, motion on, an empty toast
    /// queue, and a fresh navigator controller.
    pub fn new() -> Self {
        PlaygroundState {
            brightness: RwSignal::new(Brightness::Dark),
            reduce_motion: RwSignal::new(false),
            section: RwSignal::new(0),
            reverse: RwSignal::new(false),
            toasts: RwSignal::new(Vec::new()),
            nav: NavigatorController::new(),
            animations_enabled: RwSignal::new(true),
            last_applied_sequence: Cell::new(None),
            auto_run_db_smoke: Rc::new(Cell::new(false)),
            more_open: RwSignal::new(false),
            graph_selected: RwSignal::new(None),
            graph_transform: RwSignal::new(PanZoomTransform::IDENTITY),
            graph_controller: PanZoomController::new(),
        }
    }
}

impl Default for PlaygroundState {
    fn default() -> Self {
        Self::new()
    }
}

/// Force the app-wide theme from the current app-bar toggle flags: a
/// [`ThemeBuilder`](frust::ThemeBuilder) over [`frust_material::baseline`] with
/// the chosen brightness and the reduced-motion flag mapped into its
/// [`MotionScheme`]. Called from all three toggles so the flags
/// always compose (never clobber each other) — `reduce_motion` here is
/// already the *effective* flag (see [`effective_reduce_motion`]), not the
/// raw reduce-motion toggle.
pub(crate) fn apply_theme(brightness: Brightness, reduce_motion: bool) {
    let theme = Theme::builder(frust_material::baseline())
        .brightness(brightness)
        .map_motion(move |m: MotionScheme| MotionScheme { reduce_motion, ..m })
        .build();
    set_app_theme(theme);
}

/// The theme's effective reduced-motion flag: ON if either the reduce-motion
/// toggle is set, or the animations-off toggle is set. This is the "cheapest
/// sound wiring" the animations toggle reuses rather than inventing a parallel
/// mechanism (see [`PlaygroundState::animations_enabled`]'s doc comment) —
/// every toggle handler recomputes this from the two raw flags and feeds it
/// into [`apply_theme`].
pub(crate) fn effective_reduce_motion(reduce_motion: bool, animations_enabled: bool) -> bool {
    reduce_motion || !animations_enabled
}

/// The root [`app_bar`](frust_material::app_bar): a plug brand mark, the "playground"
/// title, and the brightness/reduce-motion/animations toggles folded into its
/// trailing actions.
fn playground_app_bar(state: &PlaygroundState) -> AnyView<PlaygroundState> {
    let brightness = state.brightness.get();
    let reduce_motion = state.reduce_motion.get();
    let animations_enabled = state.animations_enabled.get();

    let brightness_label = match brightness {
        Brightness::Dark => "\u{25d0}",
        Brightness::Light => "\u{25d1}",
    };
    let motion_label = if reduce_motion {
        "\u{23f8}"
    } else {
        "\u{25b6}"
    };
    let animations_label = if animations_enabled {
        "\u{25f5}"
    } else {
        "\u{23f9}"
    };

    // Live accent-text role — resolves per-brightness (a fixed accent color
    // can fail AA on the light surface).
    let accent = frust::use_context::<frust::Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary;

    let brand = any(icon(icons::PLUG).size(20.0).color(accent));

    let brightness_btn = any(button(brightness_label, |state: &mut PlaygroundState| {
        let next = match state.brightness.get_untracked() {
            Brightness::Dark => Brightness::Light,
            Brightness::Light => Brightness::Dark,
        };
        state.brightness.set(next);
        apply_theme(
            next,
            effective_reduce_motion(
                state.reduce_motion.get_untracked(),
                state.animations_enabled.get_untracked(),
            ),
        );
    }));

    let motion_btn = any(button(motion_label, |state: &mut PlaygroundState| {
        let next = !state.reduce_motion.get_untracked();
        state.reduce_motion.set(next);
        apply_theme(
            state.brightness.get_untracked(),
            effective_reduce_motion(next, state.animations_enabled.get_untracked()),
        );
    }));

    let animations_btn = any(button(animations_label, |state: &mut PlaygroundState| {
        let next = !state.animations_enabled.get_untracked();
        state.animations_enabled.set(next);
        apply_theme(
            state.brightness.get_untracked(),
            effective_reduce_motion(state.reduce_motion.get_untracked(), next),
        );
    }));

    any(app_bar::<PlaygroundState>("playground")
        .leading(brand)
        .actions(vec![brightness_btn, motion_btn, animations_btn]))
}

/// Bottom-center inset of the toast strip from the safe-area edge (logical
/// px).
const TOAST_MARGIN_PX: f64 = 16.0;

/// A minimal bottom-center FIFO toast strip: one muted line per pending
/// message, mounted only when the queue is non-empty so an empty queue costs
/// the tree nothing.
///
/// Hand-rolled from baseline widgets on purpose — the framework's only
/// shipped toast host belongs to a design-system catalog, and playground is
/// deliberately design-system-neutral (module docs). A page raises a toast by
/// appending to [`PlaygroundState::toasts`] from an event handler.
fn toast_overlay(pending: &[String]) -> AnyView<PlaygroundState> {
    let ink = frust::use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant;
    let lines = pending
        .iter()
        .map(|message| inflexible(any(text(message.clone()).size(12.0).color(ink))))
        .collect();
    any(Align(
        Alignment::new(0.0, 1.0),
        Padding(
            EdgeInsets::all(TOAST_MARGIN_PX),
            FlexView::new(Axis::Vertical, lines),
        ),
    ))
}

/// The narrow-width bottom nav bar: the first [`PRIMARY_NAV_COUNT`]
/// [`SECTION_LABELS`] plus a trailing "More" destination that opens
/// [`more_overlay`] instead of navigating directly. `section` is the live
/// selected index (any value — an overflow section has no primary slot of
/// its own, see below).
fn bottom_nav_bar(section: usize) -> AnyView<PlaygroundState> {
    let mut items: Vec<_> = SECTION_LABELS[..PRIMARY_NAV_COUNT]
        .iter()
        .map(|label| nav_item::<PlaygroundState>(*label))
        .collect();
    items.push(nav_item::<PlaygroundState>("More"));
    // An overflow section (index >= PRIMARY_NAV_COUNT) highlights the
    // trailing "More" slot instead of a primary one of its own — the usual
    // "More" tab behavior in a primary/overflow nav bar.
    let selected = if section < PRIMARY_NAV_COUNT {
        section
    } else {
        PRIMARY_NAV_COUNT
    };
    any(navigation_bar(
        items,
        selected,
        |state: &mut PlaygroundState, index| {
            if index < PRIMARY_NAV_COUNT {
                let current = state.section.get_untracked();
                // A move to an earlier section reads as "back": the switcher
                // plays its shared-axis slide in reverse.
                state.reverse.set(index < current);
                state.section.set(index);
            } else {
                state.more_open.set(true);
            }
        },
    ))
}

/// [`side_rail`]'s fixed column width, in logical px.
const SIDE_RAIL_WIDTH_PX: f64 = 180.0;

/// The wide-width side rail: every [`SECTION_LABELS`] destination as a
/// vertically stacked button inside a fixed-width, scrollable column, the
/// live selection highlighted via [`ButtonStyle::Primary`]. See the module
/// docs' "Responsive navigation" section for why this is hand-rolled rather
/// than `frust_material::navigation_rail`.
fn side_rail(section: usize) -> AnyView<PlaygroundState> {
    let items = SECTION_LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let style = if index == section {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            };
            inflexible(any(button(*label, move |state: &mut PlaygroundState| {
                let current = state.section.get_untracked();
                state.reverse.set(index < current);
                state.section.set(index);
            })
            .style(style)
            .small()))
        })
        .collect();
    any(
        SizedBox::<PlaygroundState>(Some(SIDE_RAIL_WIDTH_PX), None).child(scroll_view(Padding(
            EdgeInsets::all(8.0),
            FlexView::new(Axis::Vertical, items),
        ))),
    )
}

/// The narrow-width "More" destination's overlay (see the module docs'
/// "Responsive navigation" section): a bottom-anchored, opaque list of every
/// [`SECTION_LABELS`] destination plus a "Close" row. Tapping a destination
/// navigates to it and dismisses the overlay; tapping "Close" dismisses it
/// without navigating. Mounted as a top [`Stack`] layer by [`home_page`] only
/// while [`PlaygroundState::more_open`] is set.
fn more_overlay() -> AnyView<PlaygroundState> {
    let background = frust::use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .surface_container_high;
    let mut rows: Vec<_> = SECTION_LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            inflexible(any(button(*label, move |state: &mut PlaygroundState| {
                let current = state.section.get_untracked();
                state.reverse.set(index < current);
                state.section.set(index);
                state.more_open.set(false);
            })
            .style(ButtonStyle::Secondary)
            .small()))
        })
        .collect();
    rows.push(inflexible(any(button(
        "Close",
        |state: &mut PlaygroundState| state.more_open.set(false),
    )
    .style(ButtonStyle::Primary)
    .small())));
    any(Align(
        Alignment::new(0.0, 1.0),
        container(Padding(
            EdgeInsets::all(16.0),
            FlexView::new(Axis::Vertical, rows),
        ))
        .fill(background),
    ))
}

/// The navigator's home page: the root app bar over the pattern-switched
/// section body in a scroll view, with [`bottom_nav_bar`] or [`side_rail`]
/// beside/under it depending on window width (see the module docs'
/// "Responsive navigation" section), all inside one [`safe_area`] and over an
/// [`AppBackground`] base layer (see the module docs' "Mode B background"
/// section). Re-run on every rebuild (the navigator re-invokes its page
/// builder), so the signal reads here subscribe the shell to
/// section/brightness/toast/more-open changes.
fn home_page(state: &PlaygroundState) -> AnyView<PlaygroundState> {
    let section = state.section.get();
    let reverse = state.reverse.get();
    let pending = state.toasts.get();
    let more_open = state.more_open.get();

    let wide = use_context::<WindowMetrics>()
        .map(|metrics| metrics.size.width >= NAV_WIDE_BREAKPOINT_PX)
        .unwrap_or(false);

    // The pattern switcher swaps the section body whenever `section` (its key)
    // changes, playing an M3 shared-axis-X slide in the tap-derived direction.
    let handles = state.clone();
    let body = pattern_switcher(
        section,
        SharedAxis::X,
        scroll_view(pages::current(section, &handles)),
    )
    .reverse(reverse);

    // The body must be the FLEXIBLE element either way: an `inflexible` body
    // would size its container (the row at wide width, the column at narrow
    // width) to the body's own intrinsic extent, so the viewport would never
    // be smaller than the content and scrolling would never engage.
    let content: AnyView<PlaygroundState> = if wide {
        any(FlexView::new(
            Axis::Horizontal,
            vec![inflexible(side_rail(section)), flexible(1, body)],
        ))
    } else {
        any(FlexView::new(
            Axis::Vertical,
            vec![flexible(1, body), inflexible(bottom_nav_bar(section))],
        ))
    };

    // One safe area around the whole column: unlike a design-system app bar
    // that consumes the top window inset itself, the M3 `app_bar` above is a
    // plain 64dp row, so the shell owns every edge's inset here. At narrow
    // width the Material navigation bar consumes the bottom inset itself
    // (self-sizing chrome rule), so the safe area leaves that edge to it; at
    // wide width nothing else sits on that edge, so the safe area consumes it
    // instead (`.bottom(wide)`).
    let column = safe_area(FlexView::new(
        Axis::Vertical,
        vec![inflexible(playground_app_bar(state)), flexible(1, content)],
    ))
    .bottom(wide);

    // `AppBackground` is the BOTTOM-most layer (see the module docs' "Mode B
    // background" section above): under the ON `FRUST_TRANSLUCENT_SURFACE`/
    // `translucentSurface` mobile glue, an unpainted pixel anywhere in
    // `column` would otherwise be a window straight through the surface, not
    // just `platform_views`'s own deliberate slot.
    let background_color = frust::use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .surface;
    let mut layers = vec![
        any(AppBackground(background_color)),
        any(column),
        deep_link_router(state),
    ];
    if !pending.is_empty() {
        layers.push(toast_overlay(&pending));
    }
    if !wide && more_open {
        layers.push(more_overlay());
    }
    any(Stack(layers))
}

/// Builds the app-scoped [`frust_i18n::I18n`] handle the "i18n" section
/// (`pages/i18n.rs`) reads via `frust_i18n::use_i18n()`, and provides it
/// through Frust's reactive context.
///
/// Called from [`PlaygroundApp::init`] — a root `Component::init` runs under
/// the shell's own root `Owner` (`frust-core::component::Component::init`'s
/// own doc), which is exactly the "ambient reactive owner"
/// [`frust_i18n::I18n::new`]'s own doc asks a caller to construct under, the
/// same call site `clean-signals-frust`'s `provide_controller` precedent
/// uses.
///
/// Registers the ICU-backed `NUMBER`/`DATETIME` Fluent functions
/// ([`frust_i18n::fmt::with_icu_functions`]) on the `LocaleSet` **before**
/// building the [`frust_i18n::Engine`] — `locales/*/main.ftl`'s
/// `order-total`/`last-visit` messages need them. That is why this builds
/// its own `Engine` from [`locale_set`] rather than using the macro's own
/// generated `engine()`: that one is built unconfigured (no ICU functions),
/// on first use, behind a `OnceLock` — see [`crate`]'s `locales!` doc
/// comment.
fn setup_i18n() {
    let set = frust_i18n::fmt::with_icu_functions(locale_set());
    let engine = frust_i18n::Engine::new(set)
        .expect("`locales!` validated every embedded source at compile time");
    // Never fails the app: an empty/undetectable system locale list still
    // negotiates against the engine's own fallback (`en`) — see
    // `frust_i18n::I18n::from_engine`'s doc.
    let requested = frust_i18n::system_locales().unwrap_or_default();
    frust_i18n::provide_i18n(frust_i18n::I18n::from_engine(engine, &requested));
}

/// The root [`Component`]. This example's `frust::app!` call below seeds
/// Material 3 as the app's starting theme via its `setup = { .. }` block
/// (`frust::set_default_theme`, run before any shell construction) — a
/// shell's own built-in fallback is [`Theme::neutral()`], and playground
/// wants a real, design-language-neutral-enough baseline behind its plugin
/// demos; `init` additionally provides the app-scoped [`frust_i18n::I18n`]
/// handle (see [`setup_i18n`]) before wiring the reactive
/// [`PlaygroundState`].
#[derive(Default)]
pub struct PlaygroundApp;

impl Component for PlaygroundApp {
    type State = PlaygroundState;

    fn init(&self) -> PlaygroundState {
        setup_i18n();
        PlaygroundState::new()
    }

    fn build(&self, state: &mut PlaygroundState) -> AnyView<PlaygroundState> {
        // Clone the reactive handle into the navigator's stateless home-page
        // builder; the controller is shared so overlay pages push onto this
        // same stack.
        //
        // `frust::navigator` (not `frust_widgets::navigator`) — the facade
        // wrapper auto-wires Android/gesture back handling for `state.nav`,
        // so back-dismiss (overlay → pop → app exit at the root) works with
        // zero app-side back code.
        let handles = state.clone();
        any(navigator(&state.nav, move || home_page(&handles))
            .transition(TransitionSpec::duration(PageTransition::M3SharedAxisX)))
    }
}

// The generated app's sole entry point: one line binds
// `PlaygroundApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (self-gated to
// `target_os = "ios"`), and (on desktop) the hidden `__frust_main` that
// `main.rs` calls. The `setup` block seeds the Material 3 baseline as the
// app's starting theme before any shell reads the default-theme slot — a
// shell's own fallback is `Theme::neutral()`.
frust::app!(
    PlaygroundApp,
    setup = {
        frust::set_default_theme(frust_material::baseline());
    }
);

#[cfg(test)]
mod deep_link_tests {
    use super::*;

    fn link(url: &str, sequence: u64) -> DeepLink {
        DeepLink {
            url: url.to_string(),
            sequence,
        }
    }

    #[test]
    fn happy_path_db_label_navigates_and_auto_runs_smoke() {
        let db_index = pages::section_index_for("db").expect("db is a known section");
        let action = plan_deep_link(&link("frustplay://section/db", 1), None);
        assert_eq!(
            action,
            Some(DeepLinkAction::GoToSection {
                index: db_index,
                auto_run_db_smoke: true,
            })
        );
    }

    #[test]
    fn happy_path_other_label_navigates_without_auto_run() {
        let camera_index = pages::section_index_for("camera").expect("camera is a known section");
        let action = plan_deep_link(&link("frustplay://section/camera", 1), None);
        assert_eq!(
            action,
            Some(DeepLinkAction::GoToSection {
                index: camera_index,
                auto_run_db_smoke: false,
            })
        );
    }

    #[test]
    fn unknown_label_yields_unknown_label_action() {
        let action = plan_deep_link(&link("frustplay://section/nonexistent", 1), None);
        assert_eq!(
            action,
            Some(DeepLinkAction::UnknownLabel("nonexistent".to_string()))
        );
    }

    #[test]
    fn wrong_scheme_or_host_is_ignored() {
        assert_eq!(
            plan_deep_link(&link("https://section/db", 1), None),
            None,
            "wrong scheme"
        );
        assert_eq!(
            plan_deep_link(&link("frustplay://other/db", 1), None),
            None,
            "wrong host"
        );
    }

    #[test]
    fn trailing_slash_after_label_still_resolves() {
        let db_index = pages::section_index_for("db").expect("db is a known section");
        let action = plan_deep_link(&link("frustplay://section/db/", 1), None);
        assert_eq!(
            action,
            Some(DeepLinkAction::GoToSection {
                index: db_index,
                auto_run_db_smoke: true,
            })
        );
    }

    #[test]
    fn repeated_identical_url_with_new_sequence_is_applied_again() {
        let db_index = pages::section_index_for("db").expect("db is a known section");
        let first = link("frustplay://section/db", 1);
        let first_action = plan_deep_link(&first, None);
        assert!(first_action.is_some(), "first delivery must apply");

        // Same URL text, but a NEW sequence — must be applied again even
        // though `last_applied` now reflects the first delivery.
        let second = link("frustplay://section/db", 2);
        let second_action = plan_deep_link(&second, Some(first.sequence));
        assert_eq!(
            second_action,
            Some(DeepLinkAction::GoToSection {
                index: db_index,
                auto_run_db_smoke: true,
            }),
            "an identical URL with a new sequence must be applied again"
        );
    }

    #[test]
    fn same_sequence_is_not_reapplied() {
        let delivered = link("frustplay://section/db", 5);
        let action = plan_deep_link(&delivered, Some(delivered.sequence));
        assert_eq!(
            action, None,
            "a delivery whose sequence matches `last_applied` must be a no-op"
        );
    }

    #[test]
    fn parse_section_deep_link_rejects_empty_label() {
        assert_eq!(parse_section_deep_link("frustplay://section/"), None);
    }
}
