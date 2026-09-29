//! Native Widgets Demo — the showcase and device-gate vehicle for
//! `plugins/native-widgets`: one page per widget family, each real OS control
//! (`android.widget` / UIKit / AppKit) beside its frust-drawn peer, under one
//! [`Theme`] and driven by one signal. It is deliberately **not** the
//! general plugin testing ground (`examples/playground` owns that) and **not**
//! a design-system catalog (`examples/glyph-catalog` owns that).
//!
//! This module is only the **shell**: the app state, the root [`navigator`]
//! (whose home page is a Glyph [`app_bar`](frust_glyph::app_bar) carrying the
//! brightness toggle, over a
//! [`pattern_switcher`](frust::motion::switcher::pattern_switcher) hosting one
//! section page in a [`scroll_view`], with a bottom section navigation built
//! from frust's baseline [`button`]s), and the [`frust::app!`] entry binding
//! every platform. The section pages live in [`pages`] — each a
//! `page(&NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState>` (see
//! `pages/mod.rs` for the page-fn contract).
//!
//! # Why the bottom navigation is frust-drawn
//!
//! The section switcher is built from baseline [`button`]s, never the plugin's
//! native tab bar: the native tab bar is itself one of the demoed controls,
//! and off iOS/iPadOS it renders as a refusal banner — a shell navigated by it
//! would be unusable on Android and macOS. Eight sections do not fit one
//! phone-width row of labelled buttons, so the navigation is two rows of four.
//!
//! # Deep links
//!
//! `native-widgets-demo://section/<label>` routes to a section through the
//! facade's [`deep_links()`] signal (the scheme is registered in the Android
//! manifest and the iOS `Info.plist` by `frust create --deeplink-scheme`), so
//! a launcher or a device-gate script can open a page without UI interaction.
//! Labels match [`SECTION_LABELS`] case-insensitively.
//!
//! # Mode B background
//!
//! The iOS glue (`ios/Runner/SceneDelegate.swift`) turns `translucentSurface`
//! ON unless `FRUST_DEMO_OPAQUE` is set — a process-wide choice. Under Mode B
//! the surface's own base clear turns alpha-0, so every pixel no widget
//! explicitly paints is a window straight through the surface. [`home_page`]'s
//! root [`Stack`] therefore paints a surface-colored [`AppBackground`] as its
//! bottom-most layer, the same fix `examples/playground` carries.
//!
//! Run it with `cargo run` (desktop preview) or `frust run` (Android/iOS).

pub mod pages;

use std::cell::Cell;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, Widget,
};

use frust::motion::switcher::pattern_switcher;
use frust::{
    AnyView, Axis, Brightness, ButtonStyle, Color, Component, DeepLink, EdgeInsets, FlexView, Get,
    GetUntracked, NavigatorController, Padding, RwSignal, Set, SizedBox, Stack, Theme,
    TransitionSpec, Update, View, any, button, deep_links, flexible, icon, icon_button, icons,
    inflexible, navigator, safe_area, scroll_view, set_app_theme,
};
use frust_glyph::motion::{GlyphSlide, SlideDirection};

/// The section labels, in navigation order — also the deep-link slugs
/// (matched case-insensitively by [`section_index_for`]). One page per
/// native-widget family; [`pages::current`] maps an index to its page.
pub const SECTION_LABELS: [&str; 8] = [
    "Controls",
    "macOS",
    "New",
    "Alerts",
    "TabBar",
    "Sheet",
    "Composite",
    "Stress",
];

/// Map a section label (case-insensitive) to its index in
/// [`SECTION_LABELS`] — the deep-link lookup. `None` for an unknown label.
pub fn section_index_for(label: &str) -> Option<usize> {
    SECTION_LABELS
        .iter()
        .position(|&s| s.eq_ignore_ascii_case(label))
}

/// The debug-only environment variable naming the section to open at launch
/// (a [`SECTION_LABELS`] entry, case-insensitive) — the desktop shell has no
/// launch-time deep link, so this is how a scripted `cargo run` reaches a
/// page without input automation. Unset, empty or unknown: ignored (section
/// 0). Read once in [`NativeWidgetsDemoApp`]'s `init`, and only in debug
/// builds.
pub const SECTION_ENV_VAR: &str = "FRUST_DEMO_SECTION";

/// The section a [`SECTION_ENV_VAR`] value selects: `None` when unset, blank
/// or not a known label (surrounding whitespace ignored).
pub fn section_from_env_value(value: Option<&str>) -> Option<usize> {
    let label = value?.trim();
    if label.is_empty() {
        return None;
    }
    section_index_for(label)
}

// ---------------------------------------------------------------------------
// Deep-link routing — pure parse/plan functions plus the small root-mounted
// `deep_link_router` component that applies them (copied from
// `examples/playground`'s shell, minus its DB-section auto-run).
// ---------------------------------------------------------------------------

/// The scheme+host prefix every routed deep link starts with; anything else
/// (a different scheme, a different host) is not a section deep link and is
/// ignored — see [`parse_section_deep_link`].
const SECTION_DEEP_LINK_PREFIX: &str = "native-widgets-demo://section/";

/// Extract the `<label>` segment from a
/// `native-widgets-demo://section/<label>[/...]` URL. Tolerates a trailing
/// slash (and any further path/query/fragment) by taking everything up to the
/// first `/`, `?`, or `#`. Returns `None` for a non-matching scheme/host or an
/// empty label. No percent-decoding: every [`SECTION_LABELS`] entry is a plain
/// ASCII word.
fn parse_section_deep_link(url: &str) -> Option<&str> {
    let rest = url.strip_prefix(SECTION_DEEP_LINK_PREFIX)?;
    let label = rest.split(['/', '?', '#']).next().unwrap_or("");
    if label.is_empty() { None } else { Some(label) }
}

/// The effect a delivered [`DeepLink`] resolves to, as decided by
/// [`plan_deep_link`] — applied by [`deep_link_router`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeepLinkAction {
    /// Navigate to `index` (a valid [`section_index_for`] result).
    GoToSection(usize),
    /// The link named a `native-widgets-demo://section/<label>` URL, but
    /// `<label>` matched no [`SECTION_LABELS`] entry — carries the raw label
    /// for the toast/log message.
    UnknownLabel(String),
}

/// Decide what (if anything) a delivered `link` should do, given the
/// sequence of the last delivery already applied. Dedupes on
/// [`DeepLink::sequence`] — a monotonic per-delivery counter — rather than
/// comparing URL text, so two deliveries of an *identical* URL are still two
/// separate applications, while a rebuild re-observing the same delivery is a
/// no-op. Returns `None` when the delivery was already applied, or when the
/// URL is not a section link at all (a link this app does not route is not
/// this router's business).
fn plan_deep_link(link: &DeepLink, last_applied: Option<u64>) -> Option<DeepLinkAction> {
    if last_applied == Some(link.sequence) {
        return None;
    }
    let label = parse_section_deep_link(&link.url)?;
    match section_index_for(label) {
        Some(index) => Some(DeepLinkAction::GoToSection(index)),
        None => Some(DeepLinkAction::UnknownLabel(label.to_string())),
    }
}

/// A small root-mounted component, sited in [`home_page`]'s layer stack, that
/// owns the deep-link seam end to end: the single TRACKED read of
/// `frust::deep_links().latest.get()` (a `TrackedScope` re-running this
/// builder on change *is* the reactive seam), planning via
/// [`plan_deep_link`], applying the result to `state`, and recording the
/// delivery's sequence in [`NativeWidgetsDemoState::last_applied_sequence`]
/// so a later rebuild that re-observes the same delivery is a no-op. Renders
/// nothing (a zero-size [`SizedBox`]).
fn deep_link_router(state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    if let Some(link) = deep_links().latest.get() {
        let last_applied = state.last_applied_sequence.get();
        if let Some(action) = plan_deep_link(&link, last_applied) {
            match action {
                DeepLinkAction::GoToSection(index) => {
                    let current = state.section.get_untracked();
                    state.slide.set(slide_for(current, index));
                    state.section.set(index);
                    log::info!(
                        "native-widgets-demo deep-link: section {} -> {}",
                        SECTION_LABELS[index],
                        index
                    );
                }
                DeepLinkAction::UnknownLabel(label) => {
                    state.toasts.update(|queue| {
                        queue.push(format!("Unknown section '{label}' in deep link"))
                    });
                    log::warn!("native-widgets-demo deep-link: section {label} unknown");
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
/// entirely through `frust::authoring` (`examples/playground`'s
/// `AppBackground` is the identical pattern). Needed because no facade widget
/// paints an unconditional "fill available space" rect: [`Stack`] hands every
/// child a LOOSE (min-zero) constraint, so declaring a
/// larger-than-any-real-viewport intrinsic size and letting
/// [`BoxConstraints::constrain`] clamp it is the trick.
struct AppBackground(Color);

/// The retained widget for an [`AppBackground`].
struct AppBackgroundWidget(Color);

/// Declared larger than any real viewport (logical px) so
/// [`BoxConstraints::constrain`] always clamps it down to the incoming max.
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

// ---------------------------------------------------------------------------
// State + theme
// ---------------------------------------------------------------------------

/// The app's whole reactive surface, bundled so it can be cloned into the
/// navigator's home-page builder (which takes no state argument) and passed by
/// reference to every section page. The signal fields are `Copy` handles, so
/// a clone shares them.
#[derive(Clone)]
pub struct NativeWidgetsDemoState {
    /// Light/Dark, driven by the app-bar brightness toggle — the one theme
    /// signal every page's native controls and frust-drawn peers both follow.
    /// Rebuilds the theme via [`apply_theme`] and `set_app_theme`.
    pub brightness: RwSignal<Brightness>,
    /// The selected section index into [`SECTION_LABELS`]; written by the
    /// bottom navigation and by [`deep_link_router`].
    pub section: RwSignal<usize>,
    /// The slide direction the section [`pattern_switcher`] plays on the next
    /// section change, derived from the index delta (forward = content slides
    /// left).
    pub slide: RwSignal<SlideDirection>,
    /// The FIFO toast queue rendered by the root
    /// [`toast_host`](frust_glyph::toast_host); a page pushes a message by
    /// appending to it from an event handler.
    pub toasts: RwSignal<Vec<String>>,
    /// The shared navigator controller, so overlay pages can push onto the
    /// same root stack this shell mounts.
    pub nav: NavigatorController<NativeWidgetsDemoState>,
    /// The [`DeepLink::sequence`] of the last delivery [`deep_link_router`]
    /// applied, guarding against re-applying the same delivery on an
    /// unrelated rebuild. `None` means no delivery has been applied yet. A
    /// plain `Cell`: only [`deep_link_router`] touches it, always through the
    /// `state` [`home_page`] was called with.
    pub last_applied_sequence: Cell<Option<u64>>,
}

impl NativeWidgetsDemoState {
    /// Construct the initial state: section 0, dark (Glyph's dark-first
    /// default), an empty toast queue, and a fresh navigator controller.
    pub fn new() -> Self {
        NativeWidgetsDemoState {
            brightness: RwSignal::new(Brightness::Dark),
            section: RwSignal::new(0),
            slide: RwSignal::new(SlideDirection::Left),
            toasts: RwSignal::new(Vec::new()),
            nav: NavigatorController::new(),
            last_applied_sequence: Cell::new(None),
        }
    }
}

impl Default for NativeWidgetsDemoState {
    fn default() -> Self {
        Self::new()
    }
}

/// Force the app-wide theme to [`frust_glyph::baseline`] at `brightness`.
/// Glyph, not a neutral theme, so the native controls' typeface ladder has a
/// design system's bundled faces to attach (`NativeTypefaces`, ladder L3).
pub fn apply_theme(brightness: Brightness) {
    let theme = Theme::builder(frust_glyph::baseline())
        .brightness(brightness)
        .build();
    set_app_theme(theme);
}

/// The slide direction for a move from section `from` to section `to`:
/// forward (a later section) slides content left, back slides it right.
fn slide_for(from: usize, to: usize) -> SlideDirection {
    if to >= from {
        SlideDirection::Left
    } else {
        SlideDirection::Right
    }
}

// ---------------------------------------------------------------------------
// Shell chrome
// ---------------------------------------------------------------------------

/// The root [`app_bar`](frust_glyph::app_bar): a brand mark, the title, and
/// the brightness toggle. The Glyph app bar consumes the top window inset
/// itself, so the body below never pads its own top edge.
fn demo_app_bar(state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    // The mark names the brightness a press switches TO.
    let (brightness_mark, brightness_label) = match state.brightness.get() {
        Brightness::Dark => (icons::LIGHT_MODE, "Switch to light theme"),
        Brightness::Light => (icons::DARK_MODE, "Switch to dark theme"),
    };

    // Live accent role — resolves per brightness.
    let accent = frust::use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .primary;
    let brand = any(icon(icons::PLUG).size(20.0).color(accent));

    let brightness_btn = any(icon_button(
        brightness_mark,
        brightness_label,
        |state: &mut NativeWidgetsDemoState| {
            let next = match state.brightness.get_untracked() {
                Brightness::Dark => Brightness::Light,
                Brightness::Light => Brightness::Dark,
            };
            state.brightness.set(next);
            apply_theme(next);
        },
    ));

    any(
        frust_glyph::app_bar::<NativeWidgetsDemoState>("native widgets")
            .leading(brand)
            .actions(vec![brightness_btn]),
    )
}

/// Sections per navigation row — eight labelled buttons do not fit one
/// phone-width row, so the navigation is two rows of four.
const NAV_COLUMNS: usize = 4;

/// Gap around each navigation button, logical px.
const NAV_GAP_PX: f64 = 2.0;

/// The bottom section navigation: one baseline [`button`] per
/// [`SECTION_LABELS`] entry, the active section [`ButtonStyle::Primary`] and
/// the rest [`ButtonStyle::Ghost`], in rows of [`NAV_COLUMNS`]. Frust-drawn on
/// every platform — see the module docs for why it is not the native tab bar.
fn section_nav(section: usize) -> AnyView<NativeWidgetsDemoState> {
    let rows = SECTION_LABELS
        .chunks(NAV_COLUMNS)
        .enumerate()
        .map(|(row, labels)| {
            let cells = labels
                .iter()
                .enumerate()
                .map(|(col, label)| {
                    let index = row * NAV_COLUMNS + col;
                    let style = if index == section {
                        ButtonStyle::Primary
                    } else {
                        ButtonStyle::Ghost
                    };
                    let nav_button = button(*label, move |state: &mut NativeWidgetsDemoState| {
                        let current = state.section.get_untracked();
                        state.slide.set(slide_for(current, index));
                        state.section.set(index);
                    })
                    .small()
                    .style(style);
                    flexible(1, Padding(EdgeInsets::all(NAV_GAP_PX), nav_button))
                })
                .collect();
            inflexible(FlexView::new(Axis::Horizontal, cells))
        })
        .collect();
    any(Padding(
        EdgeInsets::all(NAV_GAP_PX),
        FlexView::new(Axis::Vertical, rows),
    ))
}

/// The navigator's home page: the root app bar over a safe-area'd column of
/// the pattern-switched section body (in a scroll view) and the bottom section
/// navigation, under the toast host and over an [`AppBackground`] base layer
/// (see the module docs' "Mode B background" section). Re-run on every
/// rebuild, so the signal reads here subscribe the shell to section/toast
/// changes.
fn home_page(state: &NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
    let section = state.section.get();
    let slide = state.slide.get();
    let pending = state.toasts.get();

    // The body must be the column's FLEXIBLE child: an `inflexible` child
    // would size the scroll_view to its content's intrinsic height, so
    // scrolling would never engage.
    let handles = state.clone();
    let body = flexible(
        1,
        pattern_switcher(
            section,
            GlyphSlide::new(slide),
            scroll_view(pages::current(section, &handles)),
        ),
    );

    // Bottom/left/right edges only — the app bar above already consumes the
    // top inset.
    let body_column = safe_area(FlexView::new(
        Axis::Vertical,
        vec![body, inflexible(section_nav(section))],
    ))
    .top(false);

    let column = FlexView::new(
        Axis::Vertical,
        vec![inflexible(demo_app_bar(state)), flexible(1, body_column)],
    );

    let background_color = frust::use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .surface;
    any(Stack(vec![
        any(AppBackground(background_color)),
        any(column),
        deep_link_router(state),
        any(frust_glyph::toast_host(pending)),
    ]))
}

/// The root [`Component`]. The `frust::app!` call below installs Glyph as the
/// seeded default theme (`frust_glyph::install()`, which also registers
/// Glyph's bundled fonts) before any shell construction; `init` only wires
/// the reactive [`NativeWidgetsDemoState`].
#[derive(Default)]
pub struct NativeWidgetsDemoApp;

impl Component for NativeWidgetsDemoApp {
    type State = NativeWidgetsDemoState;

    fn init(&self) -> NativeWidgetsDemoState {
        let state = NativeWidgetsDemoState::new();
        // Debug builds only: a launch-time section for scripted desktop runs
        // (see `SECTION_ENV_VAR`). Release builds never read the environment.
        #[cfg(debug_assertions)]
        if let Some(index) = section_from_env_value(std::env::var(SECTION_ENV_VAR).ok().as_deref())
        {
            state.section.set(index);
        }
        state
    }

    fn build(&self, state: &mut NativeWidgetsDemoState) -> AnyView<NativeWidgetsDemoState> {
        // `frust::navigator` auto-wires Android/gesture back handling for
        // `state.nav`, so back-dismiss works with zero app-side back code.
        let handles = state.clone();
        any(navigator(&state.nav, move || home_page(&handles)).transition(TransitionSpec::glyph()))
    }
}

// The generated app's sole entry point: one line binds
// `NativeWidgetsDemoApp` to every platform — the Android JNI exports, the iOS
// C-ABI exports, and (on desktop) the hidden `__frust_main` that `main.rs`
// calls. The `setup` block installs Glyph as the seeded default theme before
// any shell reads the default-theme slot — a shell's own fallback is
// `Theme::neutral()`, which bundles no fonts for the native controls to adopt.
frust::app!(
    NativeWidgetsDemoApp,
    setup = {
        frust_glyph::install();
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
    fn every_label_resolves_case_insensitively() {
        for (index, label) in SECTION_LABELS.iter().enumerate() {
            assert_eq!(section_index_for(label), Some(index), "{label}");
            assert_eq!(
                section_index_for(&label.to_ascii_lowercase()),
                Some(index),
                "{label} lowercased"
            );
            assert_eq!(
                section_index_for(&label.to_ascii_uppercase()),
                Some(index),
                "{label} uppercased"
            );
        }
        assert_eq!(section_index_for("nope"), None);
    }

    #[test]
    fn known_label_navigates() {
        let sheet = section_index_for("sheet").expect("Sheet is a known section");
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://section/sheet", 1), None),
            Some(DeepLinkAction::GoToSection(sheet))
        );
    }

    #[test]
    fn unknown_label_yields_unknown_label_action() {
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://section/nonexistent", 1), None),
            Some(DeepLinkAction::UnknownLabel("nonexistent".to_string()))
        );
    }

    #[test]
    fn wrong_scheme_or_host_is_ignored() {
        assert_eq!(
            plan_deep_link(&link("frustplay://section/controls", 1), None),
            None,
            "wrong scheme"
        );
        assert_eq!(
            plan_deep_link(&link("native-widgets-demo://other/controls", 1), None),
            None,
            "wrong host"
        );
    }

    #[test]
    fn trailing_slash_query_and_fragment_still_resolve() {
        let stress = section_index_for("stress").expect("Stress is a known section");
        for url in [
            "native-widgets-demo://section/Stress/",
            "native-widgets-demo://section/Stress?count=50",
            "native-widgets-demo://section/Stress#top",
        ] {
            assert_eq!(
                plan_deep_link(&link(url, 1), None),
                Some(DeepLinkAction::GoToSection(stress)),
                "{url}"
            );
        }
    }

    #[test]
    fn repeated_identical_url_with_new_sequence_is_applied_again() {
        let first = link("native-widgets-demo://section/alerts", 1);
        assert!(plan_deep_link(&first, None).is_some());
        let second = link("native-widgets-demo://section/alerts", 2);
        assert!(
            plan_deep_link(&second, Some(first.sequence)).is_some(),
            "an identical URL with a new sequence must be applied again"
        );
    }

    #[test]
    fn same_sequence_is_not_reapplied() {
        let delivered = link("native-widgets-demo://section/alerts", 5);
        assert_eq!(plan_deep_link(&delivered, Some(delivered.sequence)), None);
    }

    #[test]
    fn empty_label_is_rejected() {
        assert_eq!(
            parse_section_deep_link("native-widgets-demo://section/"),
            None
        );
    }

    #[test]
    fn section_env_value_selects_a_known_label_only() {
        let stress = section_index_for("Stress").expect("Stress is a known section");
        assert_eq!(section_from_env_value(Some("stress")), Some(stress));
        assert_eq!(section_from_env_value(Some("  Stress\n")), Some(stress));
        assert_eq!(section_from_env_value(Some("nope")), None);
        assert_eq!(section_from_env_value(Some("")), None);
        assert_eq!(section_from_env_value(None), None);
    }

    #[test]
    fn slide_direction_follows_index_delta() {
        assert_eq!(slide_for(0, 3), SlideDirection::Left);
        assert_eq!(slide_for(3, 3), SlideDirection::Left);
        assert_eq!(slide_for(5, 1), SlideDirection::Right);
    }
}
