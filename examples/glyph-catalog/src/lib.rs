//! Glyph Catalog — a standalone example app showcasing the entire Glyph
//! design system: every token scale, all 21 `frust::glyph` widgets, the
//! `Button` styles and baseline form controls under the Glyph theme, and all
//! of `frust::motion`'s transition patterns, faithful to the three vendored
//! reference builds (dark, light, motion).
//!
//! This module is only the **shell**: the app state, the root [`navigator`]
//! (whose home page is a header + glyph [`tabs`](frust::glyph::tabs) strip +
//! a [`pattern_switcher`](frust::motion::switcher::pattern_switcher) hosting
//! one of seven section pages in a [`scroll_view`], under a
//! [`toast_host`](frust::glyph::toast_host) overlay), and the [`frust::app!`]
//! entry binding all three platforms. The section pages themselves live in
//! [`pages`] — each a `page(&CatalogState) -> AnyView<CatalogState>` filled in
//! by a later task (see `pages/mod.rs` for the page-fn contract).
//!
//! Run it with `cargo run` (desktop preview) or `frust run` (Android/iOS).

pub mod pages;

use frust::motion::patterns::{GlyphSlide, SlideDirection};
use frust::motion::switcher::pattern_switcher;
use frust::{
    AnyView, Axis, Brightness, Color, Component, EdgeInsets, FlexView, Get, GetUntracked,
    MotionScheme, NavigatorController, Padding, RwSignal, Set, SizedBox, Stack, Theme,
    TransitionSpec, any, button, inflexible, navigator, scroll_view, set_app_theme, text,
};

use pages::SECTION_LABELS;

/// The catalog's whole reactive surface, bundled so it can be cloned into the
/// navigator's home-page builder (which takes no state argument) and passed by
/// reference to every section page. Every field is `Copy`/`Clone` and
/// `'static`, so [`CatalogState`] itself is `Clone` — the handle the shell
/// hands to [`pages::current`] and to overlay pushes.
///
/// All fields are reactive so a write from an event handler (a tab tap, a
/// header toggle, a page's toast trigger) wakes the shell and re-runs the
/// home-page builder.
#[derive(Clone)]
pub struct CatalogState {
    /// Light/Dark, driven by the header brightness toggle. Rebuilds the theme
    /// via [`apply_theme`] and `set_app_theme`.
    pub brightness: RwSignal<Brightness>,
    /// The OS/accessibility reduced-motion flag, driven by the header toggle;
    /// mapped into `MotionScheme::reduce_motion` by [`apply_theme`] so every
    /// transition pattern collapses to a short linear crossfade.
    pub reduce_motion: RwSignal<bool>,
    /// The selected section index (0..7); written by the tab strip.
    pub section: RwSignal<usize>,
    /// The slide direction the section [`pattern_switcher`] plays on the next
    /// section change, computed from the tab-tap index delta (forward =
    /// content slides left).
    pub slide: RwSignal<SlideDirection>,
    /// The FIFO toast queue rendered by the root
    /// [`toast_host`](frust::glyph::toast_host); a page pushes a message by
    /// appending to it.
    pub toasts: RwSignal<Vec<String>>,
    /// The shared navigator controller, so overlay pages (dialogs, the command
    /// palette, a bottom sheet — filled by `c07`) can push onto the same
    /// root stack this shell mounts.
    pub nav: NavigatorController<CatalogState>,
}

impl CatalogState {
    /// Construct the initial catalog state: section 0, dark (the Glyph
    /// dark-first default), motion on, an empty toast queue, and a fresh
    /// navigator controller.
    pub fn new() -> Self {
        CatalogState {
            brightness: RwSignal::new(Brightness::Dark),
            reduce_motion: RwSignal::new(false),
            section: RwSignal::new(0),
            slide: RwSignal::new(SlideDirection::Left),
            toasts: RwSignal::new(Vec::new()),
            nav: NavigatorController::new(),
        }
    }
}

impl Default for CatalogState {
    fn default() -> Self {
        Self::new()
    }
}

/// Force the app-wide theme from the current header-toggle flags: a
/// [`ThemeBuilder`](frust::ThemeBuilder) over [`Theme::glyph_baseline`] with
/// the chosen brightness and the reduced-motion flag mapped into its
/// [`MotionScheme`]. Called from both header toggles so the two flags always
/// compose (never clobber each other).
fn apply_theme(brightness: Brightness, reduce_motion: bool) {
    let theme = Theme::builder(Theme::glyph_baseline())
        .brightness(brightness)
        .map_motion(move |m: MotionScheme| MotionScheme { reduce_motion, ..m })
        .build();
    set_app_theme(theme);
}

/// The header: app title plus the brightness and reduce-motion toggle buttons.
/// Each toggle flips its signal AND re-applies the theme via [`apply_theme`]
/// so the swap is visible immediately.
fn header_row(state: &CatalogState) -> AnyView<CatalogState> {
    let brightness = state.brightness.get();
    let reduce_motion = state.reduce_motion.get();

    let brightness_label = match brightness {
        Brightness::Dark => "◐ Dark",
        Brightness::Light => "◑ Light",
    };
    let motion_label = if reduce_motion {
        "⏸ Motion off"
    } else {
        "▶ Motion on"
    };

    let title = inflexible(
        text("Glyph Catalog")
            .size(20.0)
            .color(Color::from_rgb8(0xFF, 0xB6, 0x27)),
    );

    let brightness_btn = inflexible(button(brightness_label, |state: &mut CatalogState| {
        let next = match state.brightness.get_untracked() {
            Brightness::Dark => Brightness::Light,
            Brightness::Light => Brightness::Dark,
        };
        state.brightness.set(next);
        apply_theme(next, state.reduce_motion.get_untracked());
    }));

    let motion_btn = inflexible(button(motion_label, |state: &mut CatalogState| {
        let next = !state.reduce_motion.get_untracked();
        state.reduce_motion.set(next);
        apply_theme(state.brightness.get_untracked(), next);
    }));

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                title,
                inflexible(SizedBox(Some(12.0), None)),
                brightness_btn,
                inflexible(SizedBox(Some(8.0), None)),
                motion_btn,
            ],
        ),
    ))
}

/// The navigator's home page: header, the 7-section tab strip, the
/// pattern-switched section body in a scroll view, all under a toast overlay.
/// Re-run on every rebuild (the navigator re-invokes its page builder), so the
/// signal reads here subscribe the shell to section/brightness/toast changes.
fn home_page(state: &CatalogState) -> AnyView<CatalogState> {
    let section = state.section.get();
    let slide = state.slide.get();
    let pending = state.toasts.get();

    let labels: Vec<String> = SECTION_LABELS.iter().map(|s| s.to_string()).collect();
    let tab_strip = inflexible(frust::glyph::tabs(
        labels,
        section,
        |state: &mut CatalogState, index| {
            let current = state.section.get_untracked();
            // Content slides left when advancing to a later tab, right when
            // going back — direction from the tab-tap index delta.
            let dir = if index >= current {
                SlideDirection::Left
            } else {
                SlideDirection::Right
            };
            state.slide.set(dir);
            state.section.set(index);
        },
    ));

    // The pattern switcher swaps the section body whenever `section` (its key)
    // changes, playing a GlyphSlide in the tap-derived direction.
    let handles = state.clone();
    let body = inflexible(pattern_switcher(
        section,
        GlyphSlide::new(slide),
        scroll_view(pages::current(section, &handles)),
    ));

    let column = FlexView::new(
        Axis::Vertical,
        vec![inflexible(header_row(state)), tab_strip, body],
    );

    // Toast host overlays the whole page (API.md's hosting note: a FIFO
    // overlay mounted above every screen).
    any(Stack(vec![
        any(column),
        any(frust::glyph::toast_host(pending)),
    ]))
}

/// The root [`Component`] (spec §5.5). The shell default theme is already
/// Glyph (every shell seeds `Theme::glyph_baseline()`), so `init` forces
/// nothing — it only wires the reactive [`CatalogState`].
#[derive(Default)]
pub struct CatalogApp;

impl Component for CatalogApp {
    type State = CatalogState;

    fn init(&self) -> CatalogState {
        CatalogState::new()
    }

    fn build(&self, state: &mut CatalogState) -> AnyView<CatalogState> {
        // Clone the reactive handle into the navigator's stateless home-page
        // builder; the controller is shared so overlay pages push onto this
        // same stack. Glyph page transitions for any pushed overlay.
        let handles = state.clone();
        any(navigator(&state.nav, move || home_page(&handles)).transition(TransitionSpec::glyph()))
    }
}

// The generated app's sole entry point (spec §5.5/§10): one line binds
// `CatalogApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (self-gated to
// `target_os = "ios"`), and (on desktop) the hidden `__frust_main` that
// `main.rs` calls.
frust::app!(CatalogApp);
