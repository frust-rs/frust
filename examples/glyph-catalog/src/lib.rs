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
    BackFallback, Brightness, Color, Component, Get, GetUntracked, MotionScheme,
    NavigatorController, RwSignal, ScrollInfo, Set, Theme, TransitionSpec, View, back_fallback,
    button, column, either, flexible, icon, icons, inflexible, navigator, safe_area, scroll_view,
    set_app_theme, stack,
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

/// A zero-size probe that publishes the window's logical width into a signal
/// from its layout pass (`LayoutCtx::window_size`), writing only on change so
/// the shell rebuilds once per resize, never per frame. The shell has no other
/// size signal; the full-screen switcher demo needs the width to choose
/// between the phone and tablet card scales.
struct WindowWidthProbe(RwSignal<f64>);

/// The retained widget for a [`WindowWidthProbe`].
struct WindowWidthProbeWidget(RwSignal<f64>);

impl<State: 'static> View<State> for WindowWidthProbe {
    type Element = WindowWidthProbeWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> WindowWidthProbeWidget {
        WindowWidthProbeWidget(self.0)
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut WindowWidthProbeWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.0 = self.0;
        ChangeFlags::NONE
    }
}

impl Widget for WindowWidthProbeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let width = ctx.window_size().width;
        if width > 0.0 && self.0.get_untracked() != width {
            self.0.set(width);
        }
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
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
    /// Whether the Motion page's card-switcher demo stage is open (the
    /// stage shows its strip instead of the live page). A signal, not a plain
    /// `bool`, so the stateless Android-back handler in
    /// [`switcher_back`](Self::switcher_back) can close the stage too.
    pub switcher_open: RwSignal<bool>,
    /// Handshake step 2 of the card-switcher demo: the peer index growing into
    /// the live page, `None` outside a selection. See `pages::motion`.
    pub switcher_expanding: RwSignal<Option<usize>>,
    /// Selection generation of the card-switcher demo: bumped by every new
    /// selection and by every close or back, so a delayed step 4 only applies
    /// to the selection that scheduled it (even a repeat of the same peer).
    pub switcher_generation: RwSignal<u64>,
    /// Which fake server face (index into the demo's four faces) is currently
    /// the live page of the card-switcher demo.
    pub switcher_live: RwSignal<usize>,
    /// Whether the full-screen card-switcher demo has taken over the window
    /// (no tab strip, no page `scroll_view`). Set by the Motion page's launch
    /// button, cleared by the demo's "Exit demo" app-bar action.
    pub switcher_fullscreen: RwSignal<bool>,
    /// The window's logical width, published by [`WindowWidthProbe`] during
    /// layout (0 until the first layout); the demo reads it to pick the
    /// phone or tablet card scale via `pages::motion::card_scale_for`.
    pub window_width: RwSignal<f64>,
    /// The card-switcher demo's back handle, created once here (it needs the
    /// reactive runtime, so never inside a `View::build`); its `on_back`
    /// closes the stage through [`switcher_open`](Self::switcher_open).
    pub switcher_back: frust_glyph::SwitcherBack,
    /// The full-screen demo's back rule, claiming while the stage is open or
    /// the demo is full screen: back closes an open stage, else it returns to
    /// the tabbed shell instead of falling through to an app exit.
    pub switcher_exit_back: BackFallback,
}

/// Withdraw a pending selection and close the stage: bump the generation (so
/// an earlier selection stays stale across a reopen), clear the expansion
/// request, clear open. The one place this three-step sequence lives; reach
/// it through [`CatalogState::close_switcher`].
fn close_switcher(
    open: RwSignal<bool>,
    expanding: RwSignal<Option<usize>>,
    generation: RwSignal<u64>,
) {
    generation.set(generation.get_untracked() + 1);
    expanding.set(None);
    open.set(false);
}

impl CatalogState {
    /// Withdraw a pending selection and close the card-switcher stage, so a
    /// delayed step 4 scheduled earlier finds nothing pending. The scrim, the
    /// close chip, Exit demo and back all end here.
    pub(crate) fn close_switcher(&self) {
        close_switcher(
            self.switcher_open,
            self.switcher_expanding,
            self.switcher_generation,
        );
    }

    /// Construct the initial catalog state: section 0, dark (the Glyph
    /// dark-first default), motion on, an empty toast queue, and a fresh
    /// navigator controller.
    pub fn new() -> Self {
        let switcher_open = RwSignal::new(false);
        let switcher_expanding = RwSignal::new(None);
        let switcher_generation = RwSignal::new(0u64);
        let switcher_fullscreen = RwSignal::new(false);
        // Back withdraws a pending selection too, so the delayed swap
        // does not switch to a server the user backed out of.
        let close = move || close_switcher(switcher_open, switcher_expanding, switcher_generation);
        CatalogState {
            brightness: RwSignal::new(Brightness::Dark),
            reduce_motion: RwSignal::new(false),
            section: RwSignal::new(0),
            slide: RwSignal::new(SlideDirection::Left),
            toasts: RwSignal::new(Vec::new()),
            nav: NavigatorController::new(),
            elevated: RwSignal::new(false),
            animations_enabled: RwSignal::new(true),
            switcher_open,
            switcher_expanding,
            switcher_generation,
            switcher_live: RwSignal::new(0),
            switcher_fullscreen,
            window_width: RwSignal::new(0.0),
            switcher_back: frust_glyph::SwitcherBack::new(close),
            switcher_exit_back: BackFallback::new(
                move || switcher_open.get_untracked() || switcher_fullscreen.get_untracked(),
                move || {
                    if switcher_open.get_untracked() {
                        close();
                    } else {
                        switcher_fullscreen.set(false);
                    }
                },
            ),
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
fn catalog_app_bar(state: &CatalogState) -> impl View<CatalogState> + use<> {
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

    let brand = icon(icons::PALETTE).size(20.0).color(accent);

    let brightness_btn = button(brightness_label, |state: &mut CatalogState| {
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
    });

    let motion_btn = button(motion_label, |state: &mut CatalogState| {
        let next = !state.reduce_motion.get_untracked();
        state.reduce_motion.set(next);
        apply_theme(
            state.brightness.get_untracked(),
            effective_reduce_motion(next, state.animations_enabled.get_untracked()),
        );
    });

    let animations_btn = button(animations_label, |state: &mut CatalogState| {
        let next = !state.animations_enabled.get_untracked();
        state.animations_enabled.set(next);
        apply_theme(
            state.brightness.get_untracked(),
            effective_reduce_motion(state.reduce_motion.get_untracked(), next),
        );
    });

    frust_glyph::app_bar::<CatalogState>("glyph catalog")
        .leading(brand)
        .actions((brightness_btn, motion_btn, animations_btn))
        .elevated(elevated)
}

/// The navigator's home page: the root AppBar over a safe-area'd body (the
/// 9-section tab strip plus the pattern-switched section body in a scroll
/// view), all under a bare toast overlay, over an [`AppBackground`] base
/// layer (see the module docs' "Mode B background" section). Re-run on every
/// rebuild (the navigator re-invokes its page builder), so the signal reads
/// here subscribe the shell to section/brightness/elevation/toast changes.
fn home_page(state: &CatalogState) -> impl View<CatalogState> + use<> {
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
    let body_column = safe_area(column().push(tab_strip).push(body)).top(false);

    let column = column().child(catalog_app_bar(state)).flex(1, body_column);

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
    stack()
        .child(AppBackground(background_color))
        .child(column)
        .child(frust_glyph::toast_host(pending))
}

/// The full-screen card-switcher demo: the stage fills the whole window
/// (below the opaque background, above no tab strip or scroll view) with the
/// width probe and the toast overlay alongside. The exit affordance is the
/// live page's "Exit demo" app-bar action (see `pages::motion`).
fn switcher_fullscreen_page(state: &CatalogState) -> impl View<CatalogState> + use<> {
    let pending = state.toasts.get();
    let background_color = frust::use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .surface;
    stack()
        .child(AppBackground(background_color))
        .child(WindowWidthProbe(state.window_width))
        .child(column().flex(1, pages::motion::switcher_stage(state)))
        .child(frust_glyph::toast_host(pending))
}

/// The shell's root page: the full-screen switcher demo when
/// [`CatalogState::switcher_fullscreen`] is set (wrapped in its
/// [`switcher_exit_back`](CatalogState::switcher_exit_back) rule), the tabbed
/// [`home_page`] otherwise.
fn shell_page(state: &CatalogState) -> impl View<CatalogState> + use<> {
    either(
        state.switcher_fullscreen.get(),
        || back_fallback(&state.switcher_exit_back, switcher_fullscreen_page(state)),
        || home_page(state),
    )
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
        navigator(&state.nav, move || shell_page(&handles)).transition(TransitionSpec::glyph())
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

#[cfg(test)]
mod tests {
    use super::*;
    use frust::accesskit::Role;
    use frust::{AnyView, any};
    use frust_core::RenderRoot;
    use frust_reactive::ReactiveRuntime;
    use frust_text::TextContext;
    use reactive_graph::owner::Owner;
    use std::sync::{Mutex, MutexGuard};

    /// Serializes the tests that mount a back-wired view: a back press bumps
    /// `frust-reactive`'s process-wide counter, which any other test thread's
    /// next back wire would consume.
    pub(crate) static BACK_PRESS_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn back_press_lock() -> MutexGuard<'static, ()> {
        BACK_PRESS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drop any live can-pop provider and the polled flag, so a back test
    /// starts with nothing claiming. Call under [`back_press_lock`] and a
    /// runtime.
    fn back_reset() {
        frust_reactive::clear_can_pop_provider();
        frust_reactive::set_handles_back(false);
    }

    /// The shell kept mounted across frames, so a back press reaches the
    /// fallbacks the previous frames wired.
    struct Shell {
        root: RenderRoot<CatalogState, AnyView<CatalogState>>,
        tcx: TextContext,
    }

    impl Shell {
        fn new() -> Self {
            Self {
                root: RenderRoot::new(),
                tcx: TextContext::new(),
            }
        }

        /// Rebuild and lay out at `(w, h)` for two frames (the second sees the
        /// width the first layout published); the semantics as (role, label).
        fn frames(&mut self, state: &mut CatalogState, w: f64, h: f64) -> Vec<(Role, String)> {
            // erasure: keep the RenderRoot root type is AnyView, shared by every shell test
            let mut logic = |s: &mut CatalogState| any(shell_page(s));
            for _ in 0..2 {
                self.root.rebuild(&mut logic, state);
                let tcx_any: &mut dyn std::any::Any = &mut self.tcx;
                self.root.layout_with_text(kurbo::Size::new(w, h), tcx_any);
            }
            self.root
                .semantics()
                .nodes
                .iter()
                .map(|(_, n)| (n.role(), n.label().unwrap_or_default().to_string()))
                .collect()
        }
    }

    /// Mount a fresh shell at `(w, h)`; see [`Shell::frames`].
    fn mount(state: &mut CatalogState, w: f64, h: f64) -> Vec<(Role, String)> {
        Shell::new().frames(state, w, h)
    }

    fn has(nodes: &[(Role, String)], role: Role, label: &str) -> bool {
        nodes.iter().any(|(r, l)| *r == role && l == label)
    }

    fn setup() -> (Owner, CatalogState) {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let owner = Owner::new();
        owner.set();
        (owner, CatalogState::new())
    }

    #[test]
    fn shell_switches_between_tabbed_and_fullscreen_stage() {
        let _back = back_press_lock();
        for (w, h, tablet) in [(390.0, 844.0, false), (1194.0, 834.0, true)] {
            let (_owner, mut state) = setup();
            let tabbed = mount(&mut state, w, h);
            assert!(
                has(&tabbed, Role::Tab, "Motion"),
                "tabbed shell has the tab strip"
            );
            assert!(!has(&tabbed, Role::Button, "Exit demo"));

            state.switcher_fullscreen.set(true);
            let full = mount(&mut state, w, h);
            assert!(
                has(&full, Role::Button, "Exit demo"),
                "full-screen stage has the exit action"
            );
            assert!(
                !has(&full, Role::Tab, "Motion"),
                "no tab strip in full-screen mode"
            );

            assert_eq!(state.window_width.get_untracked(), w);
            let scale = pages::motion::card_scale_for(state.window_width.get_untracked());
            assert_eq!(scale, tablet.then_some(0.58));

            state.switcher_fullscreen.set(false);
            assert!(has(&mount(&mut state, w, h), Role::Tab, "Motion"));
        }
    }

    #[test]
    fn back_in_the_demo_with_the_stage_closed_returns_to_the_tabbed_shell() {
        let _back = back_press_lock();
        let (_owner, mut state) = setup();
        back_reset();
        let mut shell = Shell::new();
        state.switcher_fullscreen.set(true);
        let demo = shell.frames(&mut state, 390.0, 844.0);
        assert!(has(&demo, Role::Button, "Exit demo"), "in the demo");
        assert!(
            frust_reactive::handles_back(),
            "the closed demo claims back"
        );

        frust_reactive::push_back_press();
        let after = shell.frames(&mut state, 390.0, 844.0);
        assert!(!state.switcher_fullscreen.get_untracked(), "demo left");
        assert!(has(&after, Role::Tab, "Motion"), "back on the tabbed shell");
        assert!(
            !frust_reactive::handles_back(),
            "the tabs leave back to the OS"
        );
    }

    #[test]
    fn back_in_the_demo_with_the_stage_open_closes_it_and_stays() {
        let _back = back_press_lock();
        let (_owner, mut state) = setup();
        back_reset();
        let mut shell = Shell::new();
        state.switcher_fullscreen.set(true);
        state.switcher_open.set(true);
        shell.frames(&mut state, 390.0, 844.0);
        let generation = state.switcher_generation.get_untracked();

        frust_reactive::push_back_press();
        let after = shell.frames(&mut state, 390.0, 844.0);
        assert!(!state.switcher_open.get_untracked(), "the stage closed");
        assert_eq!(state.switcher_generation.get_untracked(), generation + 1);
        assert!(
            state.switcher_fullscreen.get_untracked(),
            "still in the demo"
        );
        assert!(has(&after, Role::Button, "Exit demo"));
        assert!(!has(&after, Role::Tab, "Motion"));
    }
}
