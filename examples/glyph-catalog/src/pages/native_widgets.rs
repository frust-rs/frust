//! Native Widgets section (native-widgets p1-06, redesigned in p3-05): all
//! six v1 controls (`Button`/`Label`/`Switch`/`Slider`/`ProgressBar`/`Image`)
//! rendered from pure Rust via `frust-native-widgets`'s `frust-api` builders,
//! each one **beside its glyph (frust-drawn) counterpart** — this crate's
//! device-gate vehicle for the whole native-widgets feature (mirrors
//! `camera.rs`'s "device-gate vehicle" role for `frust-camera`).
//!
//! # Side by side: the page's whole argument
//!
//! Every row in [`page`]'s comparison section is a pair — the REAL platform
//! control on the left, frust's own glyph-themed widget on the right, in a
//! cell of **exactly the same size**, under **the same [`Theme`]**, bound to
//! **the same signal**. That is the honest form of the question this feature
//! exists to answer ("why would I use a native widget?"): a person holding
//! the phone can see a real `android.widget.Switch`'s thumb travel, a real
//! `SeekBar`'s ripple and a real `UISwitch`'s haptics against frust's own
//! drawn equivalents, rather than being told about them.
//!
//! Because both columns read and write the *same* signals ([`confirm_switch`]
//! / [`confirm_slider`] are shared by both sides), dragging the glyph slider
//! also moves the native `SeekBar` and the native `ProgressBar` — the
//! frust → native direction of the round trip, which the native → frust
//! direction below completes.
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
//! `ProgressBar` beside it (native → signal → **native**), proving the same
//! signal can fan out to more than one control.
//!
//! # Write-back affordance (closes p2-05 bar 8's gap)
//!
//! The Phase 2 device gate's echo bar could only be *partially* executed:
//! `research/VERIFY-P2.md`'s Finding 1 recorded that this page was a plain
//! signal mirror — it echoed whatever the control reported and never wrote a
//! *different* value back, so the **rejecting** round trip had no affordance
//! to exercise. [`writeback_toggle`] is that affordance: with it on, both
//! [`confirm_switch`] and [`confirm_slider`] echo the app's OWN (unchanged)
//! value back during the change handler instead of accepting the requested
//! one, which is exactly the scenario `research/RESEARCH-P2-REFRESH.md` §3's
//! Caveat B describes (a filed report that `setOn:` called from inside a
//! `valueChanged` handler can re-enter it).
//!
//! ## Why the rejection also has to move a prop (a finding, not a hack)
//!
//! A naive "echo the same value" rejection never reaches the platform at all,
//! and would have made the bar vacuous a third time. The chain: the app's
//! value did not change → the builder's `params_json` is byte-identical →
//! `frust-shell-common`'s platform-view differ emits no `UpdateParams` →
//! `NativeWidget::update` never runs → `SwitchProps::plan`'s drift branch (the
//! write-back seam, `plugins/native-widgets/src/controls/switch.rs`'s
//! *controlled-component contract*) never fires → the platform control keeps
//! the user's optimistic flip while the app believes it refused. So while
//! rejecting, and **only** while rejecting, each control's
//! `content_description` carries a monotonically increasing refusal count
//! ([`switch_description`]/[`slider_description`]). That is a real, app-owned
//! prop, so it moves the wire, which is what lets `plan` see
//! `observed != app value` and actually plan the setter — and the setter is
//! what p3-07 needs in order to test the echo guard at all. Accept mode is
//! untouched: its params are byte-for-byte what the p2 gate measured.
//!
//! The glyph column rejects too, which is the instructive contrast: frust's
//! own controlled `Switch`/`Slider` snap back within the same frame (their
//! `rebuild` re-applies the app value), while the native ones have to be
//! driven back across the wire a frame later.
//!
//! # The composite lives in the plugin, and mounts behind a toggle (p3-08)
//!
//! p3-02 shipped native-subtree support and the generic
//! `native_component(kind, component, props)` mounting builder, and this page
//! is where a reader meets a composite — one component owning a whole native
//! hierarchy frust does not lay out. [`composite_block`] mounts
//! `frust_native_widgets::DemoCard`: a parent view with a title label and two
//! buttons under it, published as **one** `platform_view` slot.
//!
//! The card itself ships inside the plugin (behind its non-default
//! `demo-components` feature), **not here**: implementing
//! `frust_native_widgets::NativeComponent` means writing per-platform view
//! construction (`ComponentCtx::new_view`/`env` against `jni::objects::JObject`
//! on Android, `objc2-ui-kit` constructors off `ComponentCtx::mtm` on iOS), and
//! this crate's `Cargo.toml` carries neither FFI crate —
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions sanction only a
//! `frust-core`/`kurbo`/`peniko` escape hatch for an `examples/*` app, and
//! `docs/DEVELOPMENT.md`'s Version-Pin Policy notes `objc2-ui-kit` is already
//! resolving at two versions in this workspace. So this page does what an app
//! actually can do with the public surface: enable the feature, register the
//! component once, and mount it. **That still does not prove a third-party
//! author could write one** — see `plugins/native-widgets/src/demo.rs`'s module
//! doc, which is honest about exactly that.
//!
//! # Exactly six native slots at rest — and seven with the composite on
//!
//! The comparison section mounts **six** `platform_view` slots and no more —
//! the number [`gate_harness_block`]'s `Total live:` readout settles back to
//! after a cycler run, which every device gate so far has keyed on. Adding a
//! seventh native slot to the always-mounted part of this page (a composite,
//! a second sample control) would change that gate constant; add it inside the
//! cycler's own group or behind a toggle instead.
//!
//! [`composite_block`]'s toggle is exactly that, and is **OFF by default** (the
//! same shape as the 50-slot stress toggle below it), so at rest this page is
//! byte-for-byte the six-slot page every prior gate measured. Turned ON,
//! `Total live:` must read **7** — ONE slot for the whole card, regardless of
//! how many native children it has. A reading of 8 or 9 would mean the card's
//! children had leaked into frust's slot space, which is the design failing;
//! the number *is* the proof that frust sees one opaque slot.
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
//!
//! # GATE HARNESS (task p1-11): a measurement rig, not a demo
//!
//! [`gate_harness_block`]'s section exists solely to give
//! `tasks/p1-10-android-device-gate.md`'s bars 5 and 6 something to measure —
//! the Phase 1 decomposition assigned those measurements to the device gate
//! but never assigned anyone to build the affordances they need. It is
//! marked GATE HARNESS in its own UI copy too (not a design-system section
//! like the ones above), and **Phase 3 may delete it outright** once the
//! on-device gate has run: a live [`frust_native_widgets::live_slot_count`]
//! readout (diagnostics-only, not a supported production API — see that
//! fn's own doc comment), a mount/unmount cycler running its OWN six-control
//! group (bar 5: proves the p1-09 teardown-retire path disposes promptly
//! rather than waiting out the differ's idle missing-streak backstop —
//! `plugins/native-widgets/src/registry.rs`'s module doc's Idle-deferred
//! dispose finding), and a 50-slot `native_label` stress toggle, off by
//! default (bar 6: the Phase 0-deferred measurement feeding PLAN's
//! shared-container escalation decision). No nested `ScrollView` here: the
//! whole "Native Widgets" page already rides one scroll view
//! (`crate::home_page`'s `scroll_view(pages::current(...))` wrapper), so
//! appending content to this page's own column is already scrollable.

use std::time::Instant;

use frust::{
    Align, Alignment, AnyView, Axis, Brightness, ButtonStyle, Color, CrossAxisAlignment,
    EdgeInsets, FlexChild, FlexView, Get, GetUntracked, Image, ImageFit, ImageSource, Padding,
    RwSignal, Set, SizedBox, Theme, any, button, checkbox, glyph, inflexible, slider, switch, text,
    use_context,
};
// Low-level escape hatch (see [`GateFrameTicker`]'s doc comment) —
// `frust-core`/`kurbo` back only that one widget below; every other widget in
// this file comes from the `frust`/`frust_native_widgets` facades. Mirrors
// `interactions.rs`'s identical `FrameTicker` escape hatch — this crate's
// `Cargo.toml` already carries both as real dependencies for that use.
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use frust_native_widgets::{
    DEMO_CARD_HEIGHT, DEMO_CARD_KIND, DEMO_CARD_WIDTH, DemoCard, DemoCardProps, NativeImageFit,
    live_slot_count, native_button, native_component, native_image, native_label, native_progress,
    native_slider, native_switch, register_demo_components,
};
use kurbo::Size;

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

/// The same embedded logo [`logo_bytes`] hands the native `ImageView`,
/// decoded ONCE for the frust-drawn [`Image`] counterpart beside it — an
/// `ImageSource` is a cheaply-clonable handle over the decoded pixels
/// (`ImageSource::same` is `Arc` pointer identity), so re-decoding per
/// rebuild would be the exact cost its own doc says never to pay.
fn logo_image() -> ImageSource {
    static LOGO: std::sync::OnceLock<ImageSource> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| {
        // A 1x1 transparent stand-in rather than a panic or an `expect`: the
        // only way this fails is a corrupt embedded asset, and a catalog demo
        // page must degrade rather than take the whole app down.
        ImageSource::decode(&logo_bytes())
            .unwrap_or_else(|_| ImageSource::from_rgba8(vec![0, 0, 0, 0], 1, 1))
    })
    .clone()
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
// Native EVENT counters, distinct from the values above (which a rejected
// change deliberately leaves untouched): p2-05 bar 8's measurable half is
// "one user interaction produces exactly ONE counter advance", and a re-entrant
// echo (`RESEARCH-P2-REFRESH.md` §3's Caveat B) would show up here as two.
local_sig!(switch_events_sig, u32, 0);
local_sig!(slider_events_sig, u32, 0);
// The glyph column's own tap counter — deliberately NOT `tap_count_sig`, which
// is the device gate's `Taps:` readout and must count native presses only.
local_sig!(glyph_tap_count_sig, u32, 0);
// Write-back affordance (p2-05 bar 8, module doc's *Write-back affordance*).
local_sig!(reject_writeback_sig, bool, false);
local_sig!(switch_refused_sig, u32, 0);
local_sig!(slider_refused_sig, u32, 0);
// The composite toggle (p3-08), OFF by default — see [`composite_block`]. This
// default is load-bearing, not a preference: ON makes the page mount a SEVENTH
// native slot, and `Total live: 6` is a gate constant every device gate since
// Phase 1 has keyed on (module doc's *Exactly six native slots at rest*).
local_sig!(composite_visible_sig, bool, false);
// GATE HARNESS state (task p1-11) — see [`gate_harness_block`]'s doc comment.
local_sig!(cycle_target_sig, u32, 0); // 0 == no cycler run started yet
local_sig!(stress_visible_sig, bool, false); // 50-slot stress toggle, off by default

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

/// A fixed-width horizontal spacer between chips in a row — [`gap`]'s
/// horizontal twin, needed by the gate harness's cycler button row (mirrors
/// `platform_views.rs`'s identical helper).
fn gap_h(w: f64) -> FlexChild<CatalogState> {
    inflexible(SizedBox(Some(w), None))
}

/// Wrap a demo's rows in a padded vertical column (one showcase block).
fn block(children: Vec<FlexChild<CatalogState>>) -> FlexChild<CatalogState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

// ---------------------------------------------------------------------------
// Side-by-side comparison chrome (p3-05)
// ---------------------------------------------------------------------------

/// One comparison column's width (logical px). Two of these plus [`PAIR_GAP`]
/// must fit inside this page's own padding on the narrowest device the gate
/// runs on — an iPhone SE is 375pt wide, and `16` (page) + `12` (block) of
/// padding on each side leaves 319pt, against `140 + 12 + 140 = 292`.
const PAIR_CELL_W: f64 = 140.0;

/// The gutter between a pair's native and glyph cells.
const PAIR_GAP: f64 = 12.0;

/// Per-control cell heights. Each is the box BOTH columns of that row get
/// (the native slot declares it, the glyph cell is boxed to it), so a pair
/// reads as one control shown twice.
///
/// Provenance: these are exactly the heights the pre-p3-05 page already gave
/// its native slots, kept unchanged so the device gate's touch targets do not
/// move. Only the *widths* changed, and only because two columns have to fit
/// ([`PAIR_CELL_W`]'s own doc).
const PAIR_BUTTON_H: f64 = 48.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_LABEL_H: f64 = 32.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_SWITCH_H: f64 = 40.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_SLIDER_H: f64 = 40.0;
/// See [`PAIR_BUTTON_H`].
const PAIR_PROGRESS_H: f64 = 24.0;
/// The image pair's square edge — both columns, so the row compares two
/// renderers of the same asset at the same size. See [`PAIR_BUTTON_H`].
const PAIR_IMAGE: f64 = 96.0;

/// How a [`pair_row`] cell treats content narrower than the cell itself.
#[derive(Clone, Copy)]
enum CellFit {
    /// Tight constraints — the control fills the whole cell. The shape for
    /// anything with a width worth comparing (a button, a slider track, a
    /// progress track).
    Stretch,
    /// Loosened constraints, content pinned to the cell's left edge — the
    /// shape for a control with a fixed intrinsic size that must NOT be
    /// stretched. `android.widget.Switch` is the load-bearing case: it is a
    /// `TextView` subclass and draws its switch graphic at the far *right* of
    /// an over-wide frame, which would put it nowhere near its glyph
    /// counterpart.
    Natural,
}

/// One `pair_row` cell: a fixed `w` x `h` box, so both columns are literally
/// the same size and the difference a reader sees is the widget, not the box.
fn cell(fit: CellFit, w: f64, h: f64, content: AnyView<CatalogState>) -> AnyView<CatalogState> {
    match fit {
        CellFit::Stretch => any(SizedBox(Some(w), Some(h)).child(content)),
        // `Align` loosens the constraints it hands its child (see its own
        // module doc), which is the only way to keep a control's natural size
        // inside a tightened box.
        CellFit::Natural => {
            any(SizedBox(Some(w), Some(h)).child(Align(Alignment::new(-1.0, 0.0), content)))
        }
    }
}

/// One comparison row: the REAL platform control on the left, its glyph
/// (frust-drawn) counterpart on the right, both in a [`cell`] of exactly the
/// same size (module doc's *Side by side*).
fn pair_row(
    title: &str,
    note: &str,
    fit: CellFit,
    height: f64,
    native: AnyView<CatalogState>,
    drawn: AnyView<CatalogState>,
) -> FlexChild<CatalogState> {
    block(vec![
        inflexible(label(title)),
        gap(4.0),
        inflexible(caption(note)),
        gap(6.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(cell(fit, PAIR_CELL_W, height, native)),
                gap_h(PAIR_GAP),
                inflexible(cell(fit, PAIR_CELL_W, height, drawn)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center))),
    ])
}

// ---------------------------------------------------------------------------
// The write-back affordance (p2-05 bar 8) — see the [module docs](self)
// ---------------------------------------------------------------------------

/// Confirm (or refuse) one requested `Switch` value, from EITHER column.
///
/// Accept mode is the plain controlled-component confirmation
/// (`docs/CODE_STANDARDS.md`'s Interaction Semantics). Reject mode writes the
/// app's own current value straight back instead — the rejecting round trip
/// p2-05 bar 8 could not exercise (module doc's *Write-back affordance*) —
/// and bumps [`switch_refused_sig`], which is what puts the refusal on the
/// wire at all.
///
/// Every read here is `*_untracked` on purpose: this runs inside an event
/// handler (a native listener on the platform main thread, or a frust
/// `on_toggle`), never inside a `build`, so it must not subscribe anything.
fn confirm_switch(requested: bool) {
    let value = switch_checked_sig();
    if reject_writeback_sig().get_untracked() {
        let refused = switch_refused_sig();
        refused.set(refused.get_untracked() + 1);
        // The echo: the app's OWN value, unchanged, written back during the
        // change handler.
        value.set(value.get_untracked());
    } else {
        value.set(requested);
    }
}

/// [`confirm_switch`]'s slider twin — same two modes, over the app-space
/// `0..=100` value both columns share.
fn confirm_slider(requested: i32) {
    let value = slider_value_sig();
    if reject_writeback_sig().get_untracked() {
        let refused = slider_refused_sig();
        refused.set(refused.get_untracked() + 1);
        value.set(value.get_untracked());
    } else {
        value.set(requested.clamp(0, 100));
    }
}

/// The native `Switch`'s accessibility label, carrying the refusal count
/// **only while rejecting**.
///
/// This is the module doc's *Why the rejection also has to move a prop*: in
/// accept mode the string is constant, so this page's params stay byte-for-byte
/// what the Phase 2 gate measured; in reject mode it changes on every refusal,
/// which is the only reason the differ emits an `UpdateParams` for a change the
/// app deliberately did not make — and therefore the only reason
/// `SwitchProps::plan`'s write-back branch runs at all.
fn switch_description(rejecting: bool, refused: u32) -> String {
    if rejecting {
        format!("Native round-trip switch \u{2014} write-back REJECT, {refused} refused")
    } else {
        "Native round-trip switch".to_string()
    }
}

/// [`switch_description`]'s slider twin, for the same reason.
fn slider_description(rejecting: bool, refused: u32) -> String {
    if rejecting {
        format!("Native round-trip slider \u{2014} write-back REJECT, {refused} refused")
    } else {
        "Native round-trip slider".to_string()
    }
}

/// The write-back mode toggle itself — labelled on-screen so someone holding
/// the phone knows what it does without reading this file (p3-05 acceptance).
fn writeback_toggle(rejecting: bool) -> AnyView<CatalogState> {
    any(checkbox(
        rejecting,
        "REJECT write-back",
        |_: &mut CatalogState, on: bool| reject_writeback_sig().set(on),
    ))
}

// ---------------------------------------------------------------------------
// The six controls, each beside its glyph counterpart
// ---------------------------------------------------------------------------

/// A native `Button`: each tap bumps [`tap_count_sig`] — the round trip's
/// first leg (native event → signal), and the device gate's `Taps:` readout.
fn button_demo() -> AnyView<CatalogState> {
    any(native_button("Tap me")
        .content_description("Native tap counter button")
        .size(PAIR_CELL_W, PAIR_BUTTON_H)
        .on_press(move || {
            let sig = tap_count_sig();
            sig.set(sig.get_untracked() + 1);
        }))
}

/// The glyph counterpart of [`button_demo`] — a frust-drawn `Button` at the
/// same size, counting into its OWN signal so the gate's `Taps:` readout
/// keeps meaning "native presses" and nothing else.
fn button_drawn() -> AnyView<CatalogState> {
    any(button("Tap me", |_: &mut CatalogState| {
        let sig = glyph_tap_count_sig();
        sig.set(sig.get_untracked() + 1);
    }))
}

/// A native `Label`: display-only, no round trip of its own — shows a fixed
/// caption so the section proves the control renders at all even with
/// nothing to demonstrate interactivity.
fn label_demo() -> AnyView<CatalogState> {
    any(native_label("Native Label")
        .content_description("A display-only native label")
        .size(PAIR_CELL_W, PAIR_LABEL_H))
}

/// The glyph counterpart of [`label_demo`] — frust's own shaped text, which
/// is where the type-scale difference between a platform `TextView` and the
/// Glyph type scale is easiest to see.
fn label_drawn() -> AnyView<CatalogState> {
    any(text("Glyph Label").size(13.0))
}

/// A native `Switch`, controlled: the app owns [`switch_checked_sig`] and
/// feeds it back in every rebuild; `on_toggle` only ever reports a
/// *requested* value (`docs/CODE_STANDARDS.md`'s Interaction Semantics),
/// which [`confirm_switch`] then confirms or refuses.
fn switch_demo(checked: bool, rejecting: bool, refused: u32) -> AnyView<CatalogState> {
    any(native_switch(checked)
        .content_description(switch_description(rejecting, refused))
        // Its natural size, not the cell's: see [`CellFit::Natural`].
        .size(70.0, PAIR_SWITCH_H)
        .on_toggle(move |requested| {
            let events = switch_events_sig();
            events.set(events.get_untracked() + 1);
            confirm_switch(requested);
        }))
}

/// The glyph counterpart of [`switch_demo`], bound to the very same signal —
/// so a flip here moves the platform `Switch` beside it (frust → native), and
/// a refusal snaps this one back within the frame while the native one has to
/// be driven back across the wire.
fn switch_drawn(checked: bool) -> AnyView<CatalogState> {
    any(switch(checked, |_: &mut CatalogState, requested: bool| {
        confirm_switch(requested)
    }))
}

/// A native `Slider`, controlled like [`switch_demo`]: a drag reports the
/// requested value through `on_change`, which [`confirm_slider`] writes into
/// [`slider_value_sig`] — the same signal [`progress_demo`] reads, so a drag
/// here moves a SECOND native control too.
fn slider_demo(value: i32, rejecting: bool, refused: u32) -> AnyView<CatalogState> {
    any(native_slider(value, 0, 100)
        .content_description(slider_description(rejecting, refused))
        .size(PAIR_CELL_W, PAIR_SLIDER_H)
        .on_change(move |requested| {
            let events = slider_events_sig();
            events.set(events.get_untracked() + 1);
            confirm_slider(requested);
        }))
}

/// The glyph counterpart of [`slider_demo`], on the same shared signal.
/// frust's `slider` is `0.0..=1.0` where the native one is app-space
/// `0..=100`, so this is the one pair that converts rather than sharing a
/// representation.
fn slider_drawn(value: i32) -> AnyView<CatalogState> {
    any(slider(
        f64::from(value) / 100.0,
        |_: &mut CatalogState, requested: f64| confirm_slider((requested * 100.0).round() as i32),
    ))
}

/// A native `ProgressBar` mirroring [`slider_value_sig`] — display-only, but
/// its value is entirely driven by the sliders above (native → signal →
/// native).
fn progress_demo(value: i32) -> AnyView<CatalogState> {
    any(native_progress(value, 0, 100)
        .content_description("Progress mirroring the slider above")
        .size(PAIR_CELL_W, PAIR_PROGRESS_H))
}

/// The glyph counterpart of [`progress_demo`] — `frust::glyph::progress`, the
/// Glyph catalog's own bar (`feedback.rs` shows it in its design-system
/// context), on the same value.
fn progress_drawn(value: i32) -> AnyView<CatalogState> {
    any(glyph::progress(f64::from(value) / 100.0))
}

/// A native `Image` showing the embedded catalog logo, cover-fit into a
/// square slot.
fn image_demo() -> AnyView<CatalogState> {
    any(native_image(logo_bytes())
        .fit(NativeImageFit::Cover)
        .content_description("Catalog logo, rendered by a native ImageView")
        .size(PAIR_IMAGE, PAIR_IMAGE))
}

/// The glyph counterpart of [`image_demo`] — the SAME bytes, decoded once by
/// [`logo_image`] and painted by frust's own `Image` at the same size and
/// fit, so the pair compares two decoders/samplers rather than two assets.
fn image_drawn() -> AnyView<CatalogState> {
    any(SizedBox(Some(PAIR_IMAGE), Some(PAIR_IMAGE))
        .child(Image(logo_image()).fit(ImageFit::Cover)))
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
// The native composite (p3-08) — see the [module docs](self)'s "The composite
// lives in the plugin" section. Unlike the GATE HARNESS region below, this is a
// real feature demo and outlives the device gate.
// ---------------------------------------------------------------------------

/// Pack a [`Color`] as the ARGB `int` every native colour setter takes — the
/// same `[a, r, g, b]` big-endian packing `frust-native-widgets`' own wire uses
/// (`plugins/native-widgets/src/api/theme.rs`'s `argb_u32`).
///
/// A component's props are typed Rust values, so folding the theme into them is
/// the **app's** job, not the builder's (`native_component`'s own doc) — this
/// is that fold, and it is why the card below re-themes live off the same
/// light/dark toggle the six controls above use.
fn argb(color: Color) -> i32 {
    let [r, g, b, a] = color.to_rgba8().to_u8_array();
    i32::from_be_bytes([a, r, g, b])
}

/// Register `DemoCard` with this thread's native-widgets runtime, once.
///
/// `register_demo_components` is first-wins and logs a warning when refused, so
/// calling it from `page` every rebuild would spam the log; the thread-local
/// latch keeps it to the one call that matters. Registration must happen on the
/// platform main thread, which a rebuild always is.
fn ensure_demo_registered() {
    thread_local! {
        static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    REGISTERED.with(|done| {
        if !done.get() {
            register_demo_components();
            done.set(true);
        }
    });
}

/// The composite itself: ONE `platform_view` slot hosting a native parent view
/// with a label and two buttons under it.
///
/// `title` is bound to live page state (the slider value) on purpose — it makes
/// the card's `update` path, and therefore its retained child handles, run on
/// device rather than only at create time. The two colours come from the active
/// [`Theme`], so the header/page brightness toggle re-themes the card through
/// the same `UpdateParams` diff as every other control here.
fn composite_demo(title: String, background: Color, ink: Color) -> AnyView<CatalogState> {
    any(native_component(
        DEMO_CARD_KIND,
        DemoCard,
        DemoCardProps {
            title,
            primary: "Primary".to_string(),
            secondary: "Dismiss".to_string(),
            background_argb: argb(background),
            title_argb: argb(ink),
        },
    )
    .size(DEMO_CARD_WIDTH, DEMO_CARD_HEIGHT)
    .interactive()
    .semantics_label("Native composite card"))
}

/// The composite's toggle + (when on) the card itself — the module doc's
/// *Exactly six native slots at rest*.
///
/// Mirrors [`gate_harness_block`]'s 50-slot stress toggle exactly: a labelled
/// `Secondary` chip flipping a page-local signal that is **off by default**, so
/// the page's resting slot count is unchanged. The copy states the expected
/// `Total live:` reading in both positions, so a person holding the phone can
/// check the one number that proves the design (7, not 9).
fn composite_block(visible: bool, slider_value: i32) -> Vec<FlexChild<CatalogState>> {
    let intro = block(vec![
        inflexible(label(
            "Native composite \u{2014} one component, one slot, three native children",
        )),
        gap(6.0),
        inflexible(caption(
            "Everything above is ONE native control per slot. This is the other shape: a single \
             `NativeComponent` that builds its own native view hierarchy (a parent with a label \
             and two buttons) and ships it as ONE `platform_view` slot \u{2014} the platform lays \
             the children out, and frust never learns they exist.",
        )),
        gap(6.0),
        inflexible(caption(
            "OFF by default because it adds a SEVENTH slot. Turn it on and \u{201c}Total live\u{201d} \
             below must read 7, not 9: one slot for the whole card however many children it has. \
             That number is the proof frust sees one opaque slot. The title tracks the slider \
             above, so its `update` path runs live; light/dark re-themes it through the same \
             UpdateParams diff as the six controls.",
        )),
        gap(6.0),
        inflexible(any(button(
            if visible {
                "Hide native composite (back to 6 slots)"
            } else {
                "Show native composite (makes it 7 slots)"
            },
            |_: &mut CatalogState| {
                let sig = composite_visible_sig();
                sig.set(!sig.get_untracked());
            },
        )
        .style(ButtonStyle::Secondary)
        .small())),
        gap(6.0),
        inflexible(caption(
            "The card is defined inside the plugin, behind its non-default `demo-components` \
             feature \u{2014} an app crate cannot implement `NativeComponent` without raw \
             jni/objc2-ui-kit dependencies of its own. This page does the part an app really can \
             do: enable the feature, register the component, mount it.",
        )),
    ]);

    let mut sections = vec![intro];
    if visible {
        // Registered lazily rather than at app init: nothing else on this page
        // needs the component, and registration is idempotent-by-latch.
        ensure_demo_registered();
        // Read only on the mounted path — a hidden card costs this page
        // nothing, which is the whole point of the default-off toggle.
        let theme = use_context::<Theme>().unwrap_or_else(Theme::glyph_baseline);
        let scheme = theme.scheme();
        sections.push(block(vec![inflexible(composite_demo(
            format!("Composite \u{2014} slider {slider_value}"),
            scheme.surface_container,
            scheme.on_surface,
        ))]));
    }
    sections
}

// ---------------------------------------------------------------------------
// GATE HARNESS (task p1-11) — a measurement rig, not a demo. See the
// [module docs](self)'s "GATE HARNESS" section. Phase 3 may delete this
// whole region outright once `tasks/p1-10-android-device-gate.md`'s bars 5/6
// have run on device.
// ---------------------------------------------------------------------------

/// How many single-control slots the bar-6 stress toggle mounts — 50, per
/// the task's own spec (`native_label`, "the cheapest control").
const STRESS_SLOT_COUNT: u32 = 50;

/// The mount/unmount cycler's per-half-step duration (mount, then unmount, is
/// two half-steps = one cycle): slow enough that a human watching the page
/// can see the group blink in and out, fast enough that a 100-cycle run (bar
/// 5's own target count) finishes in `100 * 2 * CYCLE_STEP_MS` = 30s, well
/// under a minute. **Community-approximate**: no spec pins this, it's just a
/// human-observable-but-not-glacial pace.
const CYCLE_STEP_MS: f64 = 150.0;

/// A zero-size sentinel [`View`]/[`Widget`] pair whose only job is an
/// unconditional [`PaintCtx::request_frame`] call in its own `paint` —
/// mounted only while [`cycle_state`] reports a run still has cycles left,
/// so a request traces directly to the harness that issued it. Duplicated
/// from `interactions.rs`'s identical `FrameTicker` escape hatch (each page
/// file owns its own copy per that file's established convention, not shared
/// across files) — see this crate's `Cargo.toml` for why `frust-core`/`kurbo`
/// are real dependencies already, and this file's own top-of-file comment on
/// the same import.
struct GateFrameTicker;

impl<State: 'static> View<State> for GateFrameTicker {
    type Element = GateFrameTickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> GateFrameTickerWidget {
        GateFrameTickerWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut GateFrameTickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::PAINT
    }
}

/// The retained widget for a [`GateFrameTicker`]. See its doc comment.
struct GateFrameTickerWidget;

impl Widget for GateFrameTickerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame();
    }
}

/// Mount a [`GateFrameTicker`] — call only while [`cycle_state`] reports the
/// run still has cycles left; the widget itself just asks for the next
/// frame. Mirrors `interactions.rs`'s `frame_ticker()` mount discipline: the
/// call site (not the widget) decides when a frame is actually needed.
fn gate_frame_ticker() -> FlexChild<CatalogState> {
    inflexible(GateFrameTicker)
}

thread_local! {
    /// Wall-clock start of the current mount/unmount run — `None` until the
    /// first [`start_cycle_run`] call. Reset on every subsequent call, so
    /// re-tapping a "Cycle x_" button always restarts from cycle 0 rather
    /// than continuing a stale run.
    static CYCLE_START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Start (or restart) a run of `target` mount/unmount cycles — sets
/// [`cycle_target_sig`] (which also wakes the page if it's currently idle,
/// since `page` reads it with a tracked `.get()`) and resets the wall clock
/// [`cycle_state`] reads from.
fn start_cycle_run(target: u32) {
    cycle_target_sig().set(target);
    CYCLE_START.with(|cell| cell.set(Some(Instant::now())));
}

/// Where a `target`-cycle run currently stands, purely as a function of
/// wall-clock elapsed time since [`start_cycle_run`] — the same "read a wall
/// clock directly, no discrete per-tick signal write" idiom
/// `interactions.rs`'s `demo_heartbeat` uses (this file's own escape-hatch
/// comment above), so nothing here can drift out of sync with what actually
/// painted. `target == 0` is the settled idle state (no run started yet).
///
/// Returns `(mounted, completed, running)`: whether the cycler's own
/// six-control group ([`cycle_group`]) should be showing right now, how many
/// full mount+unmount pairs have finished, and whether the run still has
/// cycles left (and therefore still needs [`gate_frame_ticker`] mounted to
/// keep advancing).
fn cycle_state(target: u32) -> (bool, u32, bool) {
    if target == 0 {
        return (false, 0, false);
    }
    let elapsed_ms = CYCLE_START.with(|cell| {
        cell.get()
            .map(|start| start.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    });
    let half_steps = (elapsed_ms / CYCLE_STEP_MS).floor() as u64;
    let total_half_steps = u64::from(target) * 2;
    if half_steps >= total_half_steps {
        // Settled: the group's final unmount already happened at the last
        // odd half-step (still `running`, below) — nothing left to advance.
        return (false, target, false);
    }
    let completed = (half_steps / 2) as u32;
    let mounted = half_steps.is_multiple_of(2);
    (mounted, completed, true)
}

/// The mount/unmount cycler's own six-control group — deliberately SEPARATE
/// instances from the six-control showcase above, so cycling never disrupts
/// that section's own round-trip demo. Each mount allocates six fresh native
/// slots; each unmount tears all six down — the p1-09 teardown-retire path
/// bar 5 exercises. Deliberately display-only (no callbacks/round-trip
/// wiring): the point is create/dispose churn, not interaction.
fn cycle_group(mounted: bool) -> AnyView<CatalogState> {
    if !mounted {
        return any(caption("(cycler group currently unmounted)"));
    }
    any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(any(native_button("Cycle button")
                .content_description("Gate-harness cycler button")
                .size(160.0, 40.0))),
            gap(4.0),
            inflexible(any(native_label("Cycle label")
                .content_description("Gate-harness cycler label")
                .size(160.0, 28.0))),
            gap(4.0),
            inflexible(any(native_switch(false)
                .content_description("Gate-harness cycler switch")
                .size(70.0, 32.0))),
            gap(4.0),
            inflexible(any(native_slider(0, 0, 100)
                .content_description("Gate-harness cycler slider")
                .size(200.0, 32.0))),
            gap(4.0),
            inflexible(any(native_progress(0, 0, 100)
                .content_description("Gate-harness cycler progress")
                .size(200.0, 20.0))),
            gap(4.0),
            inflexible(any(native_image(logo_bytes())
                .fit(NativeImageFit::Contain)
                .content_description("Gate-harness cycler image")
                .size(48.0, 48.0))),
        ],
    ))
}

/// The 50-slot stress toggle's content (bar 6): [`STRESS_SLOT_COUNT`]
/// single-control (`native_label`) slots in a plain vertical column. No
/// nested `ScrollView` here — see the [module docs](self)'s note on why the
/// whole page's own scroll view already covers this.
fn stress_grid() -> AnyView<CatalogState> {
    let rows: Vec<FlexChild<CatalogState>> = (1..=STRESS_SLOT_COUNT)
        .map(|i| {
            inflexible(any(native_label(format!("Stress slot {i:02}"))
                .content_description(format!("Gate-harness stress slot {i}"))
                .size(220.0, 28.0)))
        })
        .collect();
    any(FlexView::new(Axis::Vertical, rows))
}

/// The whole GATE HARNESS section (module doc's "GATE HARNESS" section):
/// live registry readout, mount/unmount cycler, 50-slot stress toggle. Built
/// from values [`page`] already read this rebuild (`cycle_target`/
/// `stress_visible`), matching every other section's own "compute in `page`,
/// render here" split.
fn gate_harness_block(
    live_count: usize,
    cycle_target: u32,
    cycle_completed: u32,
    cycle_mounted: bool,
    stress_visible: bool,
) -> Vec<FlexChild<CatalogState>> {
    let intro = block(vec![
        inflexible(label("GATE HARNESS \u{2014} measurement rig, not a demo")),
        gap(6.0),
        inflexible(caption(
            "Built for task p1-10's device-gate bars 5/6 (leak + lifecycle, 50-slot stress). \
             Phase 3 may delete this whole section once the on-device gate has run \u{2014} it \
             proves the registry's teardown-retire path, not a design pattern.",
        )),
    ]);

    let readout = block(vec![
        inflexible(label(
            "Live native slots (this process, every control on every page)",
        )),
        gap(4.0),
        inflexible(caption(format!("Total live: {live_count}"))),
    ]);

    let cycle_status = if cycle_target == 0 {
        "No cycle run started yet.".to_string()
    } else {
        format!(
            "Cycle {cycle_completed}/{cycle_target} \u{2014} group is currently {}",
            if cycle_mounted {
                "MOUNTED"
            } else {
                "unmounted"
            }
        )
    };
    let cycle_controls = block(vec![
        inflexible(label(
            "Mount/unmount cycler (bar 5: leak + prompt teardown)",
        )),
        gap(6.0),
        inflexible(caption(
            "Runs its OWN six-control group (separate from the showcase above), so cycling \
             never disturbs that demo. Watch \u{201c}Total live\u{201d} above dip by 6 on every \
             unmount and climb back by 6 on every mount \u{2014} it should settle back down \
             immediately, not linger for a few frames (that lag is exactly what the p1-09 \
             teardown-retire fix eliminated).",
        )),
        gap(6.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button("Cycle x5", |_: &mut CatalogState| {
                    start_cycle_run(5);
                })
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button("Cycle x100", |_: &mut CatalogState| {
                    start_cycle_run(100);
                })
                .style(ButtonStyle::Secondary)
                .small())),
            ],
        ))),
        gap(6.0),
        inflexible(caption(cycle_status)),
        gap(6.0),
        inflexible(cycle_group(cycle_mounted)),
    ]);

    let stress_toggle = block(vec![
        inflexible(label(
            "50-slot stress (bar 6: scroll-batch jank + differ batch sizes)",
        )),
        gap(6.0),
        inflexible(caption(
            "50 single-control (`native_label`) slots, off by default so the ordinary page \
             stays cheap. Scroll this whole page while it's on to observe batch sizes/jank \
             \u{2014} the Phase 0-deferred measurement PLAN's shared-container escalation \
             decision needs.",
        )),
        gap(6.0),
        inflexible(any(button(
            if stress_visible {
                "Hide 50-slot stress"
            } else {
                "Show 50-slot stress"
            },
            |_: &mut CatalogState| {
                let sig = stress_visible_sig();
                sig.set(!sig.get_untracked());
            },
        )
        .style(ButtonStyle::Secondary)
        .small())),
    ]);

    let mut sections = vec![intro, readout, cycle_controls, stress_toggle];
    if stress_visible {
        sections.push(block(vec![inflexible(stress_grid())]));
    }
    sections
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/// See the page-fn contract in [`crate::pages`]. See the [module docs](self)
/// for the full section breakdown.
pub fn page(state: &CatalogState) -> AnyView<CatalogState> {
    let taps = tap_count_sig().get();
    let checked = switch_checked_sig().get();
    let slider_value = slider_value_sig().get();
    let brightness = state.brightness.get();

    // Tracked reads, same reason as the gate-harness block below: a write from
    // a native listener (which is not a frust event pass) only wakes the page
    // because `page` subscribed to the signal here.
    let switch_events = switch_events_sig().get();
    let slider_events = slider_events_sig().get();
    let glyph_taps = glyph_tap_count_sig().get();
    let rejecting = reject_writeback_sig().get();
    let switch_refused = switch_refused_sig().get();
    let slider_refused = slider_refused_sig().get();

    // The composite toggle (p3-08) — tracked for the same reason as the gate
    // harness's own reads below.
    let composite_visible = composite_visible_sig().get();

    // GATE HARNESS (task p1-11) — tracked `.get()`s so a button tap (a
    // signal write) wakes the page even while it's otherwise idle (0fps at
    // rest), per `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions:
    // "an untracked read is a silent wake hazard".
    let cycle_target = cycle_target_sig().get();
    let stress_visible = stress_visible_sig().get();
    let (cycle_mounted, cycle_completed, cycle_running) = cycle_state(cycle_target);
    // A plain fn call, not a signal read — freshly re-evaluated every time
    // `page` itself reruns (forced continuously while `cycle_running`, via
    // `gate_frame_ticker` below).
    let live_count = live_slot_count();

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
        gap(6.0),
        inflexible(caption(
            "EVERY ROW BELOW IS A PAIR: on the LEFT the real platform control, on the RIGHT \
             frust's own glyph-drawn widget \u{2014} same cell size, same theme, same signal. \
             Drag either slider, flip either switch: both columns are wired to one value, so a \
             glyph-side change drives the native control too (frust \u{2192} native) just as a \
             native change drives the readout (native \u{2192} frust).",
        )),
    ]);

    let theme_block = block(vec![
        inflexible(label("Theme ladder (L1 brightness + L2 tokens)")),
        gap(6.0),
        inflexible(caption(
            "Flip light/dark below and watch BOTH columns re-theme LIVE \u{2014} \
             background/text colour, corner radius, and tint lists update through the same \
             UpdateParams path as any other prop change, with no remount. Only each \
             control's platform-default chrome (ripple/thumb resting colour) stays pinned to \
             whichever brightness it was first created under (L1's documented \
             approximation).",
        )),
        gap(6.0),
        inflexible(theme_toggle_demo(brightness)),
    ]);

    // The write-back affordance (p2-05 bar 8) — placed BEFORE the pairs so a
    // person gating this page picks the mode, then interacts.
    let writeback_block = block(vec![
        inflexible(label(
            "Write-back mode (p2-05 bar 8: the rejecting round trip)",
        )),
        gap(6.0),
        inflexible(caption(
            "OFF (default) = ACCEPT: the app confirms whatever the control reports, the plain \
             controlled-component contract. ON = REJECT: during the change handler the app \
             echoes its OWN unchanged value back, refusing your change. Flip it on, then drag \
             the Slider or flip the Switch in EITHER column: the glyph widget snaps back within \
             the frame, and the native one has to be driven back across the wire a frame later. \
             \u{201c}Switch events\u{201d} below must advance by exactly 1 per toggle \u{2014} 2 \
             would be the re-entrant echo RESEARCH-P2-REFRESH \u{00a7}3's Caveat B warns about.",
        )),
        gap(6.0),
        inflexible(writeback_toggle(rejecting)),
        gap(4.0),
        inflexible(caption(format!(
            "Write-back: {} \u{2014} refused so far: {switch_refused} switch, {slider_refused} \
             slider",
            if rejecting { "REJECT" } else { "ACCEPT" }
        ))),
    ]);

    let button_block = pair_row(
        "Button",
        "Native taps feed the gate's `Taps:` readout; the glyph one counts separately.",
        CellFit::Stretch,
        PAIR_BUTTON_H,
        button_demo(),
        button_drawn(),
    );

    let label_block = pair_row(
        "Label",
        "A platform TextView against frust's own shaped text, at the same box.",
        CellFit::Stretch,
        PAIR_LABEL_H,
        label_demo(),
        label_drawn(),
    );

    let switch_block = pair_row(
        "Switch",
        "One shared value. Natural size, not stretched \u{2014} a platform Switch draws its \
         graphic at the right edge of an over-wide frame.",
        CellFit::Natural,
        PAIR_SWITCH_H,
        switch_demo(checked, rejecting, switch_refused),
        switch_drawn(checked),
    );

    let slider_block = pair_row(
        "Slider",
        "One shared value, `0..=100` native / `0.0..=1.0` glyph. Drag either.",
        CellFit::Stretch,
        PAIR_SLIDER_H,
        slider_demo(slider_value, rejecting, slider_refused),
        slider_drawn(slider_value),
    );

    let progress_block = pair_row(
        "ProgressBar (mirrors the slider)",
        "Display-only on both sides \u{2014} native \u{2192} signal \u{2192} native, and \
         native \u{2192} signal \u{2192} glyph, from the one drag.",
        CellFit::Stretch,
        PAIR_PROGRESS_H,
        progress_demo(slider_value),
        progress_drawn(slider_value),
    );

    let image_block = pair_row(
        "Image",
        "The same embedded PNG bytes, cover-fit: platform ImageView vs frust's own decoder.",
        CellFit::Natural,
        PAIR_IMAGE,
        image_demo(),
        image_drawn(),
    );

    // Ordinary frust `Text` reading the same signals the controls above write
    // — the "visible frust state" half of the round trip (module doc's
    // "Interaction round-trip"). `Taps:`/`Switch:`/`Slider:` are the device
    // gate's machine-readable outputs and their spelling is load-bearing.
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
        inflexible(caption(format!("Slider: {slider_value}"))),
        gap(4.0),
        inflexible(caption(format!("Switch events: {switch_events}"))),
        inflexible(caption(format!("Slider events: {slider_events}"))),
        inflexible(caption(format!("Glyph taps: {glyph_taps}"))),
        gap(4.0),
        inflexible(caption(
            "The three values are the APP's confirmed state; the two event counters are raw \
             native listener firings (a drag fires many, a toggle must fire exactly one).",
        )),
    ]);

    let mut children = vec![
        intro,
        theme_block,
        writeback_block,
        button_block,
        label_block,
        switch_block,
        slider_block,
        progress_block,
        image_block,
        readout_block,
    ];
    // The composite goes BEFORE the gate harness: it is a feature demo that
    // outlives the harness (which Phase 3 may delete), and a person turning it
    // on scrolls straight down into the `Total live:` readout that proves it
    // cost exactly one slot.
    children.extend(composite_block(composite_visible, slider_value));
    children.extend(gate_harness_block(
        live_count,
        cycle_target,
        cycle_completed,
        cycle_mounted,
        stress_visible,
    ));
    if cycle_running {
        children.push(gate_frame_ticker());
    }

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, children),
    ))
}
