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
//! of six section pages in a [`scroll_view`], with a bottom
//! [`navigation_bar`](frust_material::navigation_bar) selecting between them, the whole
//! column inside one [`safe_area`] and under a toast overlay), and the
//! [`frust::app!`] entry binding all three platforms. The section pages
//! themselves live in [`pages`] — each a `page(&PlaygroundState) ->
//! AnyView<PlaygroundState>` (see `pages/mod.rs` for the page-fn contract).
//!
//! Back-dismiss (a pushed overlay/page, then the root navigator) works with
//! **zero** app code — `frust::navigator` (imported below) auto-wires
//! Android/gesture back handling for the shared [`NavigatorController`].
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

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, Widget,
};

use frust::motion::patterns::SharedAxis;
use frust::motion::switcher::pattern_switcher;
use frust::{
    Align, Alignment, AnyView, Axis, Brightness, Color, Component, EdgeInsets, FlexView, Get,
    GetUntracked, MotionScheme, NavigatorController, Padding, PageTransition, RwSignal, Set, Stack,
    Theme, TransitionSpec, View, any, button, flexible, icon, icons, inflexible, navigator,
    safe_area, scroll_view, set_app_theme, text,
};
use frust_material::{app_bar, nav_item, navigation_bar};

use pages::SECTION_LABELS;

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

/// The navigator's home page: the root app bar over the pattern-switched
/// section body in a scroll view, with the bottom section navigation bar
/// under it, all inside one [`safe_area`] and over an [`AppBackground`] base
/// layer (see the module docs' "Mode B background" section). Re-run on every
/// rebuild (the navigator re-invokes its page builder), so the signal reads
/// here subscribe the shell to section/brightness/toast changes.
fn home_page(state: &PlaygroundState) -> AnyView<PlaygroundState> {
    let section = state.section.get();
    let reverse = state.reverse.get();
    let pending = state.toasts.get();

    let items = SECTION_LABELS
        .iter()
        .map(|label| nav_item::<PlaygroundState>(*label))
        .collect();
    let section_bar = inflexible(navigation_bar(
        items,
        section,
        |state: &mut PlaygroundState, index| {
            let current = state.section.get_untracked();
            // A move to an earlier section reads as "back": the switcher plays
            // its shared-axis slide in reverse.
            state.reverse.set(index < current);
            state.section.set(index);
        },
    ));

    // The pattern switcher swaps the section body whenever `section` (its key)
    // changes, playing an M3 shared-axis-X slide in the tap-derived direction.
    // The body must be the column's FLEXIBLE child (flex: 1): an `inflexible`
    // child would size the scroll_view to its content's intrinsic height, so
    // the viewport would never be smaller than the content and scrolling would
    // never engage.
    let handles = state.clone();
    let body = flexible(
        1,
        pattern_switcher(
            section,
            SharedAxis::X,
            scroll_view(pages::current(section, &handles)),
        )
        .reverse(reverse),
    );

    // One safe area around the whole column: unlike a design-system app bar
    // that consumes the top window inset itself, the M3 `app_bar` above is a
    // plain 64dp row, so the shell owns every edge's inset here. The Material
    // navigation bar consumes the bottom inset itself (self-sizing chrome rule),
    // so the safe area leaves that edge to it.
    let column = safe_area(FlexView::new(
        Axis::Vertical,
        vec![inflexible(playground_app_bar(state)), body, section_bar],
    ))
    .bottom(false);

    // `AppBackground` is the BOTTOM-most layer (see the module docs' "Mode B
    // background" section above): under the ON `FRUST_TRANSLUCENT_SURFACE`/
    // `translucentSurface` mobile glue, an unpainted pixel anywhere in
    // `column` would otherwise be a window straight through the surface, not
    // just `platform_views`'s own deliberate slot.
    let background_color = frust::use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .surface;
    let mut layers = vec![any(AppBackground(background_color)), any(column)];
    if !pending.is_empty() {
        layers.push(toast_overlay(&pending));
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
