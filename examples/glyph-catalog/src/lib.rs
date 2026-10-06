//! Glyph Catalog — a standalone example app showcasing the entire Glyph
//! design system: every token scale, all 21 `frust_glyph` widgets, the
//! `Button` styles and baseline form controls under the Glyph theme, and all
//! of `frust::motion`'s transition patterns, faithful to the three vendored
//! reference builds (dark, light, motion).
//!
//! This module is only the **shell**: the app state, the root [`navigator`]
//! (whose home page is a [`glyph::app_bar`](frust_glyph::app_bar) — the
//! brand mark, "glyph catalog" title, and the brightness/reduce-motion/
//! animations toggles folded into its trailing actions (superseding the old
//! header-row `Row`) — over a [`safe_area`]'d body: the
//! 9-section tab strip plus a [`pattern_switcher`](frust::motion::switcher::pattern_switcher)
//! hosting one of nine section pages in a [`scroll_view`], under a bare
//! [`toast_host`](frust_glyph::toast_host) overlay (default bottom-center
//! anchoring, no app-side positioning)), and the
//! [`frust::app!`] entry binding all three platforms. The AppBar consumes the
//! top window inset itself, so the body's [`safe_area`] disables its own top
//! edge (`.top(false)`) to avoid double-padding it. The section pages
//! themselves live in [`pages`] — each a `page(&CatalogState) ->
//! AnyView<CatalogState>` (see `pages/mod.rs` for
//! the page-fn contract).
//!
//! Back-dismiss (a pushed overlay/page, then the root navigator) works with
//! **zero** catalog code — `frust::navigator` (imported below) auto-wires
//! Android/gesture back handling for the shared [`NavigatorController`].
//!
//! [`home_page`]'s root [`Stack`] paints an explicit, surface-colored
//! [`AppBackground`] as its bottom-most layer beneath the app content and
//! toast overlay, since [`Stack`] itself paints no implicit background.
//!
//! Run it with `cargo run` (desktop preview) or `frust run` (Android/iOS).

pub mod pages;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, Widget,
};

use frust::motion::switcher::pattern_switcher;
use frust::{
    AnyView, Axis, Brightness, Color, Component, FlexView, Get, GetUntracked, MotionScheme,
    NavigatorController, RwSignal, ScrollInfo, Set, Stack, Theme, TransitionSpec, View, any,
    button, flexible, icon, icons, inflexible, navigator, safe_area, scroll_view, set_app_theme,
};
use frust_glyph::motion::{GlyphSlide, SlideDirection};

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
    /// [`toast_host`](frust_glyph::toast_host); a page pushes a message by
    /// appending to it.
    pub toasts: RwSignal<Vec<String>>,
    /// The shared navigator controller, so overlay pages (dialogs, the command
    /// palette, a bottom sheet) can push onto the same
    /// root stack this shell mounts.
    pub nav: NavigatorController<CatalogState>,
    /// Whether the active section body has scrolled past
    /// [`ELEVATION_THRESHOLD_PX`], fed by the body `scroll_view`'s
    /// `on_scroll` and consumed by the root
    /// [`app_bar`](frust_glyph::app_bar)'s `.elevated(...)` — the
    /// same `y > 4` scrolled-shadow cue the AppBar's own reference HTML uses.
    pub elevated: RwSignal<bool>,
    /// The header animations on/off toggle (default on). OFF means "every demo
    /// renders its settled state — no frame requests anywhere": the toggle
    /// is **app-forced reduce motion + demo stop**, not a parallel
    /// mechanism — [`apply_theme`] ORs it into the same
    /// `MotionScheme::reduce_motion` push the header reduce-motion toggle
    /// drives (so every convention-following widget collapses for free),
    /// and the handful of wall-clock-driven demos on
    /// [`pages::interactions`] that bypass that theme-default collapse (see
    /// that module's "Reduced motion" docs) OR it into their own local
    /// `reduce` check instead of adding a second gating flag.
    pub animations_enabled: RwSignal<bool>,
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
            elevated: RwSignal::new(false),
            animations_enabled: RwSignal::new(true),
        }
    }
}

impl Default for CatalogState {
    fn default() -> Self {
        Self::new()
    }
}

/// Force the app-wide theme from the current header-toggle flags: a
/// [`ThemeBuilder`](frust::ThemeBuilder) over [`frust_glyph::baseline`] with
/// the chosen brightness and the reduced-motion flag mapped into its
/// [`MotionScheme`]. Called from all three header toggles so the flags
/// always compose (never clobber each other) — `reduce_motion` here is
/// already the *effective* flag (see [`effective_reduce_motion`]), not the
/// raw header reduce-motion toggle.
fn apply_theme(brightness: Brightness, reduce_motion: bool) {
    let theme = Theme::builder(frust_glyph::baseline())
        .brightness(brightness)
        .map_motion(move |m: MotionScheme| MotionScheme { reduce_motion, ..m })
        .build();
    set_app_theme(theme);
}

/// The theme's effective reduced-motion flag: ON if either the header
/// reduce-motion toggle is set, or the animations-off toggle is set. This is
/// the "cheapest sound wiring" the animations toggle reuses rather than
/// inventing a parallel mechanism (see [`CatalogState::animations_enabled`]'s
/// doc comment) — every header toggle handler recomputes this from the two
/// raw flags and feeds it into [`apply_theme`].
fn effective_reduce_motion(reduce_motion: bool, animations_enabled: bool) -> bool {
    reduce_motion || !animations_enabled
}

/// Scroll offset (logical px) past which the root AppBar grows its scrolled
/// shadow/border — mirrors the `glyph::appbar` reference HTML's `y > 4` check
/// (see [`glyph::app_bar`](frust_glyph::app_bar)'s module docs).
const ELEVATION_THRESHOLD_PX: f64 = 4.0;

/// The root [`glyph::app_bar`](frust_glyph::app_bar): a brand-mark leading
/// glyph, the "glyph catalog" title, and the brightness/reduce-motion/
/// animations toggles folded into its trailing actions — replacing the old
/// header row's own `Row` so the shell stacks exactly one bar. The
/// animations toggle is the third, added beside the other two (Ed's
/// decision — see [`CatalogState::animations_enabled`]'s doc comment for
/// what OFF means).
/// The AppBar consumes the top window inset itself (its [module
/// docs](frust_glyph::app_bar)), so the body below never pads its own top
/// edge.
fn catalog_app_bar(state: &CatalogState) -> AnyView<CatalogState> {
    let brightness = state.brightness.get();
    let reduce_motion = state.reduce_motion.get();
    let animations_enabled = state.animations_enabled.get();
    let elevated = state.elevated.get();

    let brightness_label = match brightness {
        Brightness::Dark => "◐",
        Brightness::Light => "◑",
    };
    let motion_label = if reduce_motion { "⏸" } else { "▶" };
    let animations_label = if animations_enabled { "⏵" } else { "⏹" };

    // Live accent-text role — resolves per-brightness (a fixed dark amber
    // failed AA on the light surface).
    let accent = frust::use_context::<frust::Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .primary;

    let brand = any(icon(icons::PALETTE).size(20.0).color(accent));

    let brightness_btn = any(button(brightness_label, |state: &mut CatalogState| {
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

    let motion_btn = any(button(motion_label, |state: &mut CatalogState| {
        let next = !state.reduce_motion.get_untracked();
        state.reduce_motion.set(next);
        apply_theme(
            state.brightness.get_untracked(),
            effective_reduce_motion(next, state.animations_enabled.get_untracked()),
        );
    }));

    let animations_btn = any(button(animations_label, |state: &mut CatalogState| {
        let next = !state.animations_enabled.get_untracked();
        state.animations_enabled.set(next);
        apply_theme(
            state.brightness.get_untracked(),
            effective_reduce_motion(state.reduce_motion.get_untracked(), next),
        );
    }));

    any(frust_glyph::app_bar::<CatalogState>("glyph catalog")
        .leading(brand)
        .actions(vec![brightness_btn, motion_btn, animations_btn])
        .elevated(elevated))
}

/// The navigator's home page: the root AppBar over a safe-area'd body (the
/// 9-section tab strip plus the pattern-switched section body in a scroll
/// view), all under a bare toast overlay, over an [`AppBackground`] base
/// layer (see the module docs' "Mode B background" section). Re-run on every
/// rebuild (the navigator re-invokes its page builder), so the signal reads
/// here subscribe the shell to section/brightness/elevation/toast changes.
fn home_page(state: &CatalogState) -> AnyView<CatalogState> {
    let section = state.section.get();
    let slide = state.slide.get();
    let pending = state.toasts.get();

    let labels: Vec<String> = SECTION_LABELS.iter().map(|s| s.to_string()).collect();
    let tab_strip = inflexible(frust_glyph::tabs(
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
    // changes, playing a GlyphSlide in the tap-derived direction. The body
    // must be the column's FLEXIBLE child (flex: 1): an `inflexible` child
    // would size the scroll_view to its content's intrinsic height, so the
    // viewport would never be smaller than the content and scrolling would
    // never engage (found on-device 2026-07-23). `on_scroll` feeds the root
    // AppBar's `elevated` flag off the body's own offset.
    let handles = state.clone();
    let body = flexible(
        1,
        pattern_switcher(
            section,
            GlyphSlide::new(slide),
            scroll_view(pages::current(section, &handles)).on_scroll(
                move |state: &mut CatalogState, info: ScrollInfo| {
                    state.elevated.set(info.offset > ELEVATION_THRESHOLD_PX);
                },
            ),
        ),
    );

    // Bottom/left/right edges only — the AppBar above already consumes the
    // top inset, so padding it again here would double-pad (module docs).
    let body_column = safe_area(FlexView::new(Axis::Vertical, vec![tab_strip, body])).top(false);

    let column = FlexView::new(
        Axis::Vertical,
        vec![inflexible(catalog_app_bar(state)), flexible(1, body_column)],
    );

    // Toast host overlays the whole page — a FIFO overlay mounted above
    // every screen. A bare `toast_host` anchors itself
    // bottom-center with inset-aware margins — no app-side `Align`/`Padding`
    // wrapper needed (framework-side anchoring).
    //
    // `AppBackground` is the BOTTOM-most layer (see the module docs above):
    // `Stack` itself paints no implicit background, so this is what gives
    // `column` an opaque, surface-colored backdrop.
    let background_color = frust::use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .surface;
    any(Stack(vec![
        any(AppBackground(background_color)),
        any(column),
        any(frust_glyph::toast_host(pending)),
    ]))
}

/// The root [`Component`]. This example's `frust::app!` call below installs
/// Glyph as the seeded default theme via its `setup = { .. }` block
/// (`frust_glyph::install()`, run before any shell construction) — a
/// shell's own built-in fallback is [`Theme::neutral()`] now, so `init`
/// relies on that explicit install rather than a shell-seeded Glyph default;
/// `init` itself forces nothing — it only wires the reactive
/// [`CatalogState`].
#[derive(Default)]
pub struct CatalogApp;

impl Component for CatalogApp {
    type State = CatalogState;

    fn init(&self) -> CatalogState {
        CatalogState::new()
    }

    fn build(&self, state: &mut CatalogState) -> impl View<CatalogState> {
        // Clone the reactive handle into the navigator's stateless home-page
        // builder; the controller is shared so overlay pages push onto this
        // same stack. Glyph page transitions for any pushed overlay.
        //
        // `frust::navigator` (not `frust_widgets::navigator`) — the facade
        // wrapper auto-wires Android/gesture back handling for `state.nav`,
        // so back-dismiss (overlay → pop → app
        // exit at the root) works with zero catalog-side back code.
        let handles = state.clone();
        any(navigator(&state.nav, move || home_page(&handles)).transition(TransitionSpec::glyph()))
    }
}

// The generated app's sole entry point: one line binds
// `CatalogApp` to all three platforms — the Android JNI exports
// (`target_os = "android"` only), the iOS C-ABI exports (self-gated to
// `target_os = "ios"`), and (on desktop) the hidden `__frust_main` that
// `main.rs` calls. The `setup` block installs Glyph as the seeded default
// theme (`frust_glyph::install()`) before any shell reads the
// default-theme slot — a shell's own fallback is `Theme::neutral()` now, and
// this catalog exists specifically to showcase Glyph, so it must not launch
// neutral.
frust::app!(
    CatalogApp,
    setup = {
        frust_glyph::install();
    }
);
