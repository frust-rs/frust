//! Native Widgets section (native-widgets p1-06): all six v1 controls
//! (`Button`/`Label`/`Switch`/`Slider`/`ProgressBar`/`Image`) rendered from
//! pure Rust via `frust-native-widgets`'s `frust-api` builders — this
//! crate's device-gate vehicle for the whole native-widgets feature (mirrors
//! `camera.rs`'s "device-gate vehicle" role for `frust-camera`).
//!
//! # Interaction round-trip
//!
//! Three controls are interactive (`Button`/`Switch`/`Slider`), and each
//! wires its native event straight into an `RwSignal` write — the
//! events-as-signals idiom `frust_native_widgets::api`'s module docs describe
//! ("write the given signal"). [`page`]'s own readout block reads those same
//! signals back into ordinary frust `Text`, so a tap/toggle/drag on the
//! REAL platform widget becomes visible frust state one frame later — the
//! "native event → signal → visible frust state" round trip this page
//! exists to demonstrate. The slider's value additionally drives the native
//! `ProgressBar` below it (native → signal → **native**), proving the same
//! signal can fan out to more than one control.
//!
//! # Desktop safety / the translucency-refused fallback
//!
//! No native host exists on desktop (`platform_views.rs`'s own Desktop
//! safety note) — every slot below renders nothing there under the normal
//! (non-refused) branch. Separately, `frust_native_widgets::api`'s builders
//! consult `frust::resolved_surface_mode()` themselves and degrade to a
//! frust-drawn placeholder on `RefusedTranslucent` (this catalog's own Mode B
//! testbed, `crate`'s module doc's "Mode B background" section, turns
//! translucency ON process-wide) — nothing on this page needs to special-case
//! that; it is the builders' own contract.
//!
//! # Theme ladder (L1 brightness pinning + L2 token pinning, p1-07)
//!
//! [`theme_toggle_demo`] is a page-local light/dark toggle proving the six
//! controls above re-theme LIVE: `frust_native_widgets::api`'s builders read
//! `use_context::<Theme>()` every rebuild and fold the resolved tokens
//! (background/text/accent colour, corner radius, text size) into each
//! control's `params_json`, so a `set_app_theme` write (spike 4a's proven
//! re-run mechanism, `crates/frust/tests/theme_reactivity_spike.rs`) repaints
//! every mounted native view through the ordinary `UpdateParams` diff path —
//! no remount, no flicker of a fresh platform view. It mirrors the header's
//! own brightness toggle (`crate::catalog_app_bar`) exactly, kept as its own
//! row here so a person gating this page doesn't need to scroll to the
//! header to exercise it. **L1's own effect is the one thing this toggle does
//! NOT demonstrate live**: the night-qualified `Context` a control's
//! platform-default chrome (ripple/thumb resting colour) resolves against is
//! baked at construction, so it stays pinned to whichever brightness the
//! control was first created under — a documented approximation
//! (`plugins/native-widgets/src/android/theme.rs`'s module doc), not a bug.

use frust::{
    AnyView, Axis, Brightness, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Padding,
    RwSignal, Set, SizedBox, Theme, any, button, inflexible, text, use_context,
};
use frust_native_widgets::{
    NativeImageFit, native_button, native_image, native_label, native_progress, native_slider,
    native_switch,
};

use crate::CatalogState;

/// The embedded catalog logo — published once (via [`logo_bytes`]'s
/// `OnceLock` cache) rather than re-`Arc::from`-ing `include_bytes!`'s bytes
/// every rebuild, which would mint a fresh `Arc` (and therefore a fresh
/// publish revision) on every frame — exactly the per-rebuild re-decode
/// `crate::controls::image`'s module doc says never to pay for.
fn logo_bytes() -> std::sync::Arc<[u8]> {
    static LOGO: std::sync::OnceLock<std::sync::Arc<[u8]>> = std::sync::OnceLock::new();
    std::sync::Arc::clone(
        LOGO.get_or_init(|| std::sync::Arc::from(include_bytes!("../../assets/logo.png").to_vec())),
    )
}

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (`appbar.rs`/`interactions.rs`/
/// `motion.rs`/`platform_views.rs`'s macro of the same shape), not shared
/// across files.
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

local_sig!(tap_count_sig, u32, 0);
local_sig!(switch_checked_sig, bool, false);
local_sig!(slider_value_sig, i32, 30);

// ---------------------------------------------------------------------------
// Section chrome (see appbar.rs/interactions.rs/motion.rs/platform_views.rs's
// identical helpers — duplicated per-module by established convention, not
// shared)
// ---------------------------------------------------------------------------

/// Live-theme accent-text role (`primary`), falling back to the Glyph
/// baseline pre-context.
fn amber() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .on_surface_variant
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

/// Wrap a demo's rows in a padded vertical column (one showcase block).
fn block(children: Vec<FlexChild<CatalogState>>) -> FlexChild<CatalogState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

// ---------------------------------------------------------------------------
// The six controls
// ---------------------------------------------------------------------------

/// A native `Button`: each tap bumps [`tap_count_sig`] — the round trip's
/// first leg (native event → signal).
fn button_demo() -> AnyView<CatalogState> {
    any(native_button("Tap me")
        .content_description("Native tap counter button")
        .size(160.0, 48.0)
        .on_press(move || {
            let sig = tap_count_sig();
            sig.set(sig.get_untracked() + 1);
        }))
}

/// A native `Label`: display-only, no round trip of its own — shows a fixed
/// caption so the section proves the control renders at all even with
/// nothing to demonstrate interactivity.
fn label_demo() -> AnyView<CatalogState> {
    any(native_label("Native Label")
        .content_description("A display-only native label")
        .size(200.0, 32.0))
}

/// A native `Switch`, controlled: the app owns [`switch_checked_sig`] and
/// feeds it back in every rebuild; [`Self::on_toggle`] only ever reports a
/// *requested* value (`docs/CODE_STANDARDS.md`'s Interaction Semantics).
fn switch_demo(checked: bool) -> AnyView<CatalogState> {
    any(native_switch(checked)
        .content_description("Native round-trip switch")
        .size(70.0, 40.0)
        .on_toggle(move |requested| switch_checked_sig().set(requested)))
}

/// A native `Slider`, controlled like [`switch_demo`]: a drag reports the
/// requested value through [`Self::on_change`], which this page writes
/// straight into [`slider_value_sig`] — the same signal [`progress_demo`]
/// below reads, so a drag here moves a SECOND native control too.
fn slider_demo(value: i32) -> AnyView<CatalogState> {
    any(native_slider(value, 0, 100)
        .content_description("Native round-trip slider")
        .size(260.0, 40.0)
        .on_change(move |requested| slider_value_sig().set(requested)))
}

/// A native `ProgressBar` mirroring [`slider_value_sig`] — display-only, but
/// its value is entirely driven by the slider above (native → signal →
/// native).
fn progress_demo(value: i32) -> AnyView<CatalogState> {
    any(native_progress(value, 0, 100)
        .content_description("Progress mirroring the slider above")
        .size(260.0, 24.0))
}

/// A native `Image` showing the embedded catalog logo, cover-fit into a
/// square slot.
fn image_demo() -> AnyView<CatalogState> {
    any(native_image(logo_bytes())
        .fit(NativeImageFit::Cover)
        .content_description("Catalog logo, rendered by a native ImageView")
        .size(96.0, 96.0))
}

/// The theme-ladder toggle row (module doc's *Theme ladder* section): flips
/// [`CatalogState::brightness`] and forces the app-wide theme through
/// `crate::apply_theme`/`frust::set_app_theme` — the exact mechanism the
/// header's own brightness toggle uses (`crate::catalog_app_bar`'s
/// `brightness_btn`), so every native control above re-resolves its theme
/// tokens on the very next rebuild.
fn theme_toggle_demo(brightness: Brightness) -> AnyView<CatalogState> {
    let label_text = match brightness {
        Brightness::Dark => "\u{25d0} Dark \u{2014} tap for Light",
        Brightness::Light => "\u{25d1} Light \u{2014} tap for Dark",
    };
    any(button(label_text, |state: &mut CatalogState| {
        let next = match state.brightness.get_untracked() {
            Brightness::Dark => Brightness::Light,
            Brightness::Light => Brightness::Dark,
        };
        state.brightness.set(next);
        crate::apply_theme(
            next,
            crate::effective_reduce_motion(
                state.reduce_motion.get_untracked(),
                state.animations_enabled.get_untracked(),
            ),
        );
    }))
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/// See the page-fn contract in [`crate::pages`]. See the [module docs](self)
/// for the full section breakdown.
pub fn page(state: &CatalogState) -> AnyView<CatalogState> {
    let taps = tap_count_sig().get();
    let checked = switch_checked_sig().get();
    let slider = slider_value_sig().get();
    let brightness = state.brightness.get();

    let intro = block(vec![
        inflexible(label("Native Widgets: real platform views from pure Rust")),
        gap(6.0),
        inflexible(caption(
            "Six controls below (`Button`/`Label`/`Switch`/`Slider`/`ProgressBar`/`Image`), each \
             ONE `platform_view` slot behind `frust-native-widgets`'s `frust-api` builders \u{2014} \
             no per-control Kotlin/Swift. No native host exists on desktop, so each slot renders \
             nothing there (or a frust-drawn placeholder if this app's Mode B translucency is \
             refused by the platform).",
        )),
    ]);

    let theme_block = block(vec![
        inflexible(label("Theme ladder (L1 brightness + L2 tokens)")),
        gap(6.0),
        inflexible(caption(
            "Flip light/dark below and watch every control above re-theme LIVE \u{2014} \
             background/text colour, corner radius, and tint lists update through the same \
             UpdateParams path as any other prop change, with no remount. Only each \
             control's platform-default chrome (ripple/thumb resting colour) stays pinned to \
             whichever brightness it was first created under (L1's documented \
             approximation).",
        )),
        gap(6.0),
        inflexible(theme_toggle_demo(brightness)),
    ]);

    let button_block = block(vec![
        inflexible(label("Button")),
        gap(6.0),
        inflexible(button_demo()),
    ]);

    let label_block = block(vec![
        inflexible(label("Label")),
        gap(6.0),
        inflexible(label_demo()),
    ]);

    let switch_block = block(vec![
        inflexible(label("Switch")),
        gap(6.0),
        inflexible(switch_demo(checked)),
    ]);

    let slider_block = block(vec![
        inflexible(label("Slider")),
        gap(6.0),
        inflexible(slider_demo(slider)),
    ]);

    let progress_block = block(vec![
        inflexible(label("ProgressBar (mirrors the slider)")),
        gap(6.0),
        inflexible(progress_demo(slider)),
    ]);

    let image_block = block(vec![
        inflexible(label("Image")),
        gap(6.0),
        inflexible(image_demo()),
    ]);

    // Ordinary frust `Text` reading the same three signals the controls
    // above write — the "visible frust state" half of the round trip
    // (module doc's "Interaction round-trip").
    let readout_block = block(vec![
        inflexible(label(
            "Round-trip readout (native event \u{2192} signal \u{2192} frust)",
        )),
        gap(4.0),
        inflexible(caption(format!("Taps: {taps}"))),
        inflexible(caption(format!(
            "Switch: {}",
            if checked { "ON" } else { "OFF" }
        ))),
        inflexible(caption(format!("Slider: {slider}"))),
    ]);

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                intro,
                theme_block,
                button_block,
                label_block,
                switch_block,
                slider_block,
                progress_block,
                image_block,
                readout_block,
            ],
        ),
    ))
}
