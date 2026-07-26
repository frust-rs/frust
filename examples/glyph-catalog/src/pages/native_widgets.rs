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
    AnyView, Axis, Brightness, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get,
    GetUntracked, Padding, RwSignal, Set, SizedBox, Theme, any, button, inflexible, text,
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
    NativeImageFit, live_slot_count, native_button, native_image, native_label, native_progress,
    native_slider, native_switch,
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
    let slider = slider_value_sig().get();
    let brightness = state.brightness.get();

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

    let mut children = vec![
        intro,
        theme_block,
        button_block,
        label_block,
        switch_block,
        slider_block,
        progress_block,
        image_block,
        readout_block,
    ];
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
