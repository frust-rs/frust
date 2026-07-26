//! Platform Views section (platform-views task 10): exercises the real
//! [`frust::platform_view`] machinery — the framework's task-02/07/08/09
//! native-view embedding contract — through the real machinery, superseding
//! the abandoned platform-views spike branch's throwaway page (this task's
//! own Notes: "keep the spike page OUT").
//!
//! # Mode B testbed
//!
//! This catalog is the framework's Mode B (translucent-surface) testbed: its
//! Android/iOS glue overrides the embedding module's `translucentSurface`
//! seam (`docs/ARCHITECTURE.md`'s Embedding distribution) — see
//! `android/app/src/main/kotlin/it/f0x/glyphcatalog/MainActivity.kt`'s
//! `FrustActivity.translucentSurface` override and
//! `ios/Runner/SceneDelegate.swift`'s `MainViewController.translucentSurface`
//! override, both flipped ON by this task — which turns the WHOLE app's
//! frust surface non-opaque, process-wide — not scoped to this one page.
//! [`platform_view`]'s own module docs state the resulting paint
//! contract plainly: "any region this slot's parent doesn't paint over is a
//! window straight through to the native view (or the OS background) behind
//! it". The root shell (`crate::lib`'s `home_page`) now paints an explicit
//! surface-colored background behind every page for exactly this reason —
//! this page's own [`mode_b_slot`] is the ONE deliberate hole in that
//! backdrop, not an accident.
//!
//! # Sections (this task's Details)
//! 1. [`mode_b_slot`] — a 300×400 `dev.frust.DemoStreamFactory` slot with two
//!    corner buttons overlapping its bounds (one opaque, one 50%-alpha — the
//!    alpha-over-alpha fringing probe device task 11 checks on iOS).
//! 2. A self-update proof strip explaining that the slot's own counter runs
//!    on its own Choreographer/Handler (iOS: `Timer`) loop, independent of
//!    any frust frame — this page paints nothing per-frame itself (task 11's
//!    zero-frust-frames trace target).
//! 3. Stress toggles: show/hide the slot (a real dispose/create cycle, driven
//!    by conditionally mounting/unmounting the widget — not a visibility
//!    flag), bump its `params_json` (the `updateParams` path), and a second,
//!    independent slot on/off (multi-slot).
//! 4. ≥64 filler rows below (the M1 device-viewport lesson — see
//!    `appbar.rs`'s `filler_rows` doc comment for the full story) so the slot
//!    scrolls fully off-screen and back, exercising the paint-time
//!    visible-rect culling [`platform_view`]'s own module docs describe ("a
//!    culled subtree publishes no frame at all").
//!
//! # Desktop safety
//!
//! No native host exists on desktop: [`PlatformViewView::debug_fill`]
//! (debug builds only — the method doesn't exist under a release profile,
//! hence [`maybe_debug_fill`]'s own `#[cfg]` split) paints a translucent
//! magenta slab over each slot's bounds so the reserved region is visible
//! without one; a release desktop build paints nothing there, matching the
//! widget's documented v1 contract.

use frust::motion::AnimatedOpacity;
use frust::{
    Align, Alignment, AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get,
    GetUntracked, Padding, PlatformViewView, ProgressValue, RwSignal, Set, SizedBox, Stack, Theme,
    any, button, circular_progress, inflexible, platform_view, text, use_context,
};

use crate::CatalogState;

/// The Mode B slot's native factory — see `android/.../dev/frust/DemoStreamFactory.kt`
/// / `ios/Runner/DemoStreamFactory.swift`. The two platforms name a
/// `platform_view` factory differently by convention
/// (`crates/frust-widgets/src/platform_view.rs`'s module docs: Android's
/// `viewType` is the fully-qualified `dev.frust.<Name>Factory` class name;
/// iOS's is the bare `@objc(<Name>)` runtime name — ObjC class names carry no
/// package/dot syntax, matching the template's own debug factory example
/// (`"FrustTestLabelFactory"`, not `"dev.frust.FrustTestLabelFactory"`)), so
/// this constant is target-gated rather than a single shared literal.
#[cfg(target_os = "android")]
const DEMO_STREAM_VIEW_TYPE: &str = "dev.frust.DemoStreamFactory";
#[cfg(target_os = "ios")]
const DEMO_STREAM_VIEW_TYPE: &str = "DemoStreamFactory";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const DEMO_STREAM_VIEW_TYPE: &str = "dev.frust.DemoStreamFactory";

/// The Mode B slot's fixed size (this task's Details: "a 300×400 slot").
const SLOT_W: f64 = 300.0;
const SLOT_H: f64 = 400.0;

/// The second (multi-slot stress toggle) slot's size — deliberately smaller/
/// distinct from [`SLOT_W`]/[`SLOT_H`] so the two are visually distinguishable
/// on device.
const SECOND_SLOT_W: f64 = 220.0;
const SECOND_SLOT_H: f64 = 160.0;

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (`appbar.rs`/`interactions.rs`/
/// `motion.rs`'s macro of the same shape), not shared across files.
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

local_sig!(slot_visible_sig, bool, true); // dispose/create stress toggle
local_sig!(params_bump_sig, u32, 0); // updateParams stress toggle
local_sig!(second_slot_visible_sig, bool, false); // multi-slot stress toggle

// --- THROWAWAY: native-widgets Phase 0 spike signals (spikes 1/3/4b) --------
local_sig!(spike_visible_sig, bool, false); // spike button slot mount toggle
local_sig!(spike_stress_sig, bool, false); // 50-control + 500 sets/frame stress
local_sig!(spike_click_bump_sig, i64, 0); // performClick trigger (round-trip)
local_sig!(spike_bars_sig, bool, false); // spike 4b: hide/show system bars
local_sig!(spike_clicks_sig, u64, 0); // written by the NATIVE click handler
local_sig!(spike_overlay_clicks_sig, u64, 0); // frust overlay button (z-shield bar)

/// The spike factory's view type — Android FQCN convention (see
/// [`DEMO_STREAM_VIEW_TYPE`]'s doc for the naming law).
#[cfg(target_os = "android")]
const SPIKE_VIEW_TYPE: &str = "dev.frust.FrustNativeSpikeFactory";
#[cfg(target_os = "ios")]
const SPIKE_VIEW_TYPE: &str = "FrustNativeSpikeFactory";
#[cfg(not(any(target_os = "android", target_os = "ios")))]
const SPIKE_VIEW_TYPE: &str = "dev.frust.FrustNativeSpikeFactory";

// ---------------------------------------------------------------------------
// Section chrome (see appbar.rs/interactions.rs/motion.rs's identical helpers)
// ---------------------------------------------------------------------------

/// Live-theme accent-text role (`primary`), falling back to the Glyph
/// baseline pre-context — see `motion.rs`'s `amber()` twin.
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
/// horizontal twin, needed by the stress-toggle button row.
fn gap_h(w: f64) -> FlexChild<CatalogState> {
    inflexible(SizedBox(Some(w), None))
}

/// Wrap a demo's rows in a padded vertical column (one showcase block) —
/// mirrors `appbar.rs`/`interactions.rs`/`motion.rs`'s helper of the same
/// name/shape.
fn block(children: Vec<FlexChild<CatalogState>>) -> FlexChild<CatalogState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

/// A bounded list of filler rows — the scrollable content that lets the Mode
/// B slot scroll fully off-screen and back (this task's Details point 4).
/// Duplicated locally per `appbar.rs`'s own precedent (private, per-module,
/// not shared): 64 rows, deliberately — at ~31 logical px per row this is
/// ~2000px of content, comfortably taller than any phone viewport (the M1
/// device-viewport lesson: a shorter filler barely exceeded a tall device's
/// body height, so scrolling read as broken on-device while the same filler
/// scrolled generously in a shorter headless test viewport, masking the bug).
fn filler_rows() -> AnyView<CatalogState> {
    let rows: Vec<AnyView<CatalogState>> = (1..=64)
        .map(|i| {
            any(Padding(
                EdgeInsets::symmetric(0.0, 8.0),
                text(format!("row {i:02} \u{2014} platform-views filler")).size(12.0),
            ))
        })
        .collect();
    any(FlexView::new(
        Axis::Vertical,
        rows.into_iter().map(inflexible).collect(),
    ))
}

// ---------------------------------------------------------------------------
// Mode B slot(s)
// ---------------------------------------------------------------------------

/// Apply [`PlatformViewView::debug_fill`] only in a debug build (see the
/// [module docs](self)'s Desktop safety section) — the method itself doesn't
/// exist under `#[cfg(debug_assertions)]` on the builder, so a plain
/// conditional reassignment would leave an "unused `mut`" lint in release
/// builds; this free function sidesteps that.
fn maybe_debug_fill(view: PlatformViewView) -> PlatformViewView {
    #[cfg(debug_assertions)]
    {
        view.debug_fill()
    }
    #[cfg(not(debug_assertions))]
    {
        view
    }
}

/// The Mode B section's primary slot: a 300×400 `DemoStreamFactory` with two
/// corner buttons overlapping its bounds via `Align` (a `Stack`'s children
/// are always sized/positioned against the stack's own box, so `Align`
/// pinning a small button to a corner reads as chrome overlapping the slot's
/// edges while staying inside its bounds) — one opaque
/// ([`ButtonStyle::Primary`]), one wrapped in
/// [`frust::motion::AnimatedOpacity`] at a static `0.5` (never retargeted, so
/// it settles immediately with no ongoing animation — the alpha-over-alpha
/// fringing probe this task's Details call out, static so it doesn't cost a
/// frame request at rest). `bump` feeds the slot's `params_json` — the
/// updateParams stress toggle's payload.
fn mode_b_slot(bump: u32) -> AnyView<CatalogState> {
    let params = format!("{{\"bump\":{bump}}}");
    let slot = maybe_debug_fill(
        platform_view(DEMO_STREAM_VIEW_TYPE)
            .size(SLOT_W, SLOT_H)
            .params_json(params)
            .semantics_label("Demo native stream (Mode B slot)"),
    );

    let opaque_btn = button("Opaque", |_: &mut CatalogState| {})
        .style(ButtonStyle::Primary)
        .small();
    let alpha_btn = AnimatedOpacity(
        0.5,
        button("50% alpha", |_: &mut CatalogState| {})
            .style(ButtonStyle::Primary)
            .small(),
    );

    any(Stack(vec![
        any(slot),
        any(Align(Alignment::TOP_LEFT, opaque_btn)),
        any(Align(Alignment::BOTTOM_RIGHT, alpha_btn)),
    ]))
}

/// The second, independent slot (multi-slot stress toggle) — smaller, no
/// overlapping chrome (the fringing probe above already covers that), just
/// proof that two live `platform_view` slots coexist (each gets its own
/// `slot_id` per widget instance — `platform_view.rs`'s module docs' Identity
/// section — so reusing the same [`DEMO_STREAM_VIEW_TYPE`] factory for both is
/// fine: it's resolved/cached once per `viewType` on the native side, then
/// instantiated once per slot).
fn second_slot() -> AnyView<CatalogState> {
    let slot = maybe_debug_fill(
        platform_view(DEMO_STREAM_VIEW_TYPE)
            .size(SECOND_SLOT_W, SECOND_SLOT_H)
            .params_json("{\"slot\":\"second\"}")
            .semantics_label("Second demo native stream slot"),
    );
    any(slot)
}

// ---------------------------------------------------------------------------
// THROWAWAY: native-widgets Phase 0 spike section (spikes 1/3/4b)
// ---------------------------------------------------------------------------

/// The spike's native-button slot: a Rust-JNI-built `android.widget.Button`
/// behind the Mode B surface. `click` bumps ride `params_json` and trigger
/// `performClick` native-side — the round-trip proof that needs no touch
/// forwarding. Spike 3 layers the REAL tap path on top: the slot is marked
/// `.interactive()`, and a frust "Overlay" button pinned to the slot's
/// top-right sits inside a matching `.shield_local` region — the z-shield
/// bar (a tap there must reach the FRUST button, not the native sibling).
fn spike_button_slot(click_bump: i64, bars: bool) -> AnyView<CatalogState> {
    let params = format!(
        "{{\"control\":\"button\",\"id\":1,\"click\":{click_bump},\"bars\":{}}}",
        i32::from(bars)
    );
    let slot = platform_view(SPIKE_VIEW_TYPE)
        .size(240.0, 56.0)
        .params_json(params)
        .interactive()
        .shield_local(kurbo::Rect::new(150.0, 0.0, 240.0, 40.0))
        .semantics_label("Native spike button (Rust-built)");
    let overlay = button("Ovl", |_: &mut CatalogState| {
        let sig = spike_overlay_clicks_sig();
        sig.set(sig.get_untracked() + 1);
    })
    .style(ButtonStyle::Primary)
    .small();
    // Constrain the Stack to exactly the slot's box, so TOP_RIGHT genuinely
    // overlaps the native button's right end (the z-shield bar) instead of
    // landing beside it in a wider flex row.
    any(SizedBox(Some(240.0), Some(56.0)).child(any(Stack(vec![
        any(slot),
        any(Align(
            Alignment::TOP_RIGHT,
            SizedBox(Some(90.0), Some(40.0)).child(any(overlay)),
        )),
    ]))))
}

/// The spike's stress slot: 50 Rust-JNI-built `TextView`s in one container.
/// While mounted, [`spike_stress_area`]'s builder performs 500 direct property
/// sets per rebuild via [`frust_native_widgets_spike::stress_tick`].
fn spike_stress_slot() -> AnyView<CatalogState> {
    any(platform_view(SPIKE_VIEW_TYPE)
        .size(260.0, 220.0)
        .params_json("{\"control\":\"stress\",\"id\":100}")
        .semantics_label("Native spike stress hierarchy"))
}

/// The stress area: the slot + an indeterminate spinner whose cosmetic loop
/// keeps frames running (each Run frame rebuilds this page, and this builder
/// calls `stress_tick(500)` — the exact Phase 1 rebuild-time property-set
/// path). µs numbers land in logcat as `spike-perf stress ...` lines.
fn spike_stress_area() -> AnyView<CatalogState> {
    let us = frust_native_widgets_spike::stress_tick(500);
    let readout = match us {
        Some(us) => format!("last batch: 500 sets in {us}\u{b5}s (see logcat spike-perf)"),
        None => "stress hierarchy not live yet \u{2014} first frame creates it".to_string(),
    };
    any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(any(FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(any(SizedBox(Some(16.0), Some(16.0))
                        .child(any(circular_progress(ProgressValue::Indeterminate))))),
                    gap_h(8.0),
                    inflexible(caption(readout)),
                ],
            ))),
            gap(6.0),
            inflexible(spike_stress_slot()),
        ],
    ))
}

/// The whole spike section, mounted below the existing demo sections.
fn spike_section(
    visible: bool,
    stress: bool,
    click_bump: i64,
    bars: bool,
    clicks: u64,
) -> FlexChild<CatalogState> {
    let mut rows = vec![
        inflexible(label("Native-widgets spike (Phase 0, throwaway)")),
        inflexible(caption(format!(
            "native clicks seen by Rust: {clicks} \u{2014} overlay (frust) clicks: {} \u{2014} live spike refs: {}",
            spike_overlay_clicks_sig().get(),
            frust_native_widgets_spike::live_ref_count()
        ))),
        gap(6.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button(
                    if visible { "Unmount" } else { "Mount" },
                    |_: &mut CatalogState| {
                        let sig = spike_visible_sig();
                        sig.set(!sig.get_untracked());
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button("Sim click", |_: &mut CatalogState| {
                    let sig = spike_click_bump_sig();
                    sig.set(sig.get_untracked() + 1);
                })
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button(
                    if stress { "Stress off" } else { "Stress on" },
                    |_: &mut CatalogState| {
                        let sig = spike_stress_sig();
                        sig.set(!sig.get_untracked());
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button(
                    if bars { "Bars show" } else { "Bars hide" },
                    |_: &mut CatalogState| {
                        let sig = spike_bars_sig();
                        sig.set(!sig.get_untracked());
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
            ],
        ))),
        gap(8.0),
    ];
    if visible {
        rows.push(inflexible(spike_button_slot(click_bump, bars)));
    } else {
        rows.push(inflexible(caption(
            "spike button unmounted \u{2014} refs above must read 0 with stress off",
        )));
    }
    if stress {
        rows.push(gap(8.0));
        rows.push(inflexible(spike_stress_area()));
    }
    block(rows)
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/// See the page-fn contract in [`crate::pages`]. See the [module docs](self)
/// for the full section breakdown.
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    let slot_visible = slot_visible_sig().get();
    let bump = params_bump_sig().get();
    let second_visible = second_slot_visible_sig().get();

    // THROWAWAY spike state + the native-event handler registration (a signal
    // write on the platform main thread — the exact "native event → signal →
    // one frame" chain spike 1 measures).
    let spike_visible = spike_visible_sig().get();
    let spike_stress = spike_stress_sig().get();
    let spike_click_bump = spike_click_bump_sig().get();
    let spike_bars = spike_bars_sig().get();
    let spike_clicks = spike_clicks_sig().get();
    frust_native_widgets_spike::set_click_handler(|_control_id, _kind| {
        let sig = spike_clicks_sig();
        sig.set(sig.get_untracked() + 1);
    });
    // Spike 2 (iOS): force the Rust `define_class!` factory's lazy ObjC
    // registration during rebuild — strictly before the host can poll the
    // first `create` command (see the spike crate's apple module doc).
    #[cfg(target_os = "ios")]
    frust_native_widgets_spike::apple::ensure_registered();

    let intro = block(vec![
        inflexible(label("Mode B: native-view compositing")),
        inflexible(caption(
            "a 300\u{d7}400 dev.frust.DemoStreamFactory slot below, chrome overlapping its \
             edges \u{2014} one opaque button, one 50% alpha (the alpha-over-alpha fringing \
             probe). The app bar above stays; everywhere else on this page paints its own \
             background \u{2014} no accidental windows.",
        )),
    ]);

    let toggles = block(vec![
        inflexible(label("Stress toggles")),
        gap(6.0),
        inflexible(any(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(button(
                    if slot_visible {
                        "Hide slot"
                    } else {
                        "Show slot"
                    },
                    |_: &mut CatalogState| {
                        let sig = slot_visible_sig();
                        sig.set(!sig.get_untracked());
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button(
                    format!("Bump params ({bump})"),
                    |_: &mut CatalogState| {
                        let sig = params_bump_sig();
                        sig.set(sig.get_untracked() + 1);
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button(
                    if second_visible {
                        "2nd slot: on"
                    } else {
                        "2nd slot: off"
                    },
                    |_: &mut CatalogState| {
                        let sig = second_slot_visible_sig();
                        sig.set(!sig.get_untracked());
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
            ],
        ))),
    ]);

    // Show/hide is a real dispose/create cycle: the widget is conditionally
    // mounted/unmounted (not a `visible` flag), so hiding actually tears down
    // the `PlatformViewWidget` (the native `dispose` command fires) and
    // showing again allocates a fresh `slot_id` from scratch.
    let slot_area = block(vec![if slot_visible {
        inflexible(mode_b_slot(bump))
    } else {
        inflexible(any(SizedBox(Some(SLOT_W), Some(80.0)).child(caption(
            "slot disposed \u{2014} tap \u{201c}Show slot\u{201d} to recreate",
        ))))
    }]);

    let proof_strip = block(vec![
        inflexible(label("Self-update proof")),
        inflexible(caption(
            "the slot's counter ticks \u{223c}20Hz on its OWN Choreographer/Handler \
             (iOS: Timer) loop \u{2014} independent of any frust frame. This page paints \
             nothing per-frame; the frame gate should read 0fps at rest even while the \
             counter keeps moving (device-verified in task 11).",
        )),
    ]);

    let mut children = vec![intro, toggles, slot_area];
    if second_visible {
        children.push(block(vec![
            inflexible(label("Second slot (multi-slot)")),
            gap(6.0),
            inflexible(second_slot()),
        ]));
    }
    children.push(spike_section(
        spike_visible,
        spike_stress,
        spike_click_bump,
        spike_bars,
        spike_clicks,
    ));
    children.push(proof_strip);
    children.push(gap(12.0));
    children.push(inflexible(filler_rows()));

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, children),
    ))
}
