//! Platform Views section: exercises the real
//! [`frust::platform_view`] machinery — the framework's
//! native-view embedding contract.
//!
//! # Mode B testbed
//!
//! Playground is the framework's Mode B (translucent-surface) testbed: its
//! Android/iOS glue overrides the embedding module's `translucentSurface`
//! seam (`docs/ARCHITECTURE.md`'s Embedding distribution) — see
//! `android/app/src/main/kotlin/it/f0x/playground/MainActivity.kt`'s
//! `FrustActivity.translucentSurface` override and
//! `ios/Runner/SceneDelegate.swift`'s `MainViewController.translucentSurface`
//! override, both flipped ON — which turns the WHOLE app's
//! frust surface non-opaque, process-wide — not scoped to this one page.
//! [`platform_view`]'s own module docs state the resulting paint
//! contract plainly: "any region this slot's parent doesn't paint over is a
//! window straight through to the native view (or the OS background) behind
//! it". The root shell (`crate::lib`'s `home_page`) now paints an explicit
//! surface-colored background behind every page for exactly this reason —
//! this page's own [`mode_b_slot`] is the ONE deliberate hole in that
//! backdrop, not an accident.
//!
//! # Sections
//! 1. [`mode_b_slot`] — a 300×400 `dev.frust.DemoStreamFactory` slot with two
//!    corner buttons overlapping its bounds (one opaque, one 50%-alpha — an
//!    alpha-over-alpha fringing probe for an on-device check on iOS).
//! 2. A self-update proof strip explaining that the slot's own counter runs
//!    on its own Choreographer/Handler (iOS: `Timer`) loop, independent of
//!    any frust frame — this page paints nothing per-frame itself (the
//!    zero-frust-frames property an on-device trace confirms).
//! 3. Stress toggles: show/hide the slot (a real dispose/create cycle, driven
//!    by conditionally mounting/unmounting the widget — not a visibility
//!    flag), bump its `params_json` (the `updateParams` path), and a second,
//!    independent slot on/off (multi-slot).
//! 4. ≥64 filler rows below (the device-viewport lesson — see
//!    [`filler_rows`]'s own doc comment for the full story) so the slot
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
    GetUntracked, Padding, PlatformViewView, RwSignal, Set, SizedBox, Stack, Theme, any, button,
    inflexible, platform_view, text, use_context,
};

use crate::PlaygroundState;

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

/// The Mode B slot's fixed size.
const SLOT_W: f64 = 300.0;
const SLOT_H: f64 = 400.0;

/// The second (multi-slot stress toggle) slot's size — deliberately smaller/
/// distinct from [`SLOT_W`]/[`SLOT_H`] so the two are visually distinguishable
/// on device.
const SECOND_SLOT_W: f64 = 220.0;
const SECOND_SLOT_H: f64 = 160.0;

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner — the
/// established per-module precedent (the demo app's `composite.rs` page
/// carries the same macro), not shared across files.
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

// ---------------------------------------------------------------------------
// Section chrome (every page file carries its own copy of these helpers by
// established convention, not a shared module)
// ---------------------------------------------------------------------------

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink — see [`accent`]'s twin.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

/// A demo heading in the accent role.
fn label(s: impl Into<String>) -> AnyView<PlaygroundState> {
    any(text(s).size(13.0).color(accent()))
}

/// A muted per-demo caption.
fn caption(s: impl Into<String>) -> AnyView<PlaygroundState> {
    any(text(s).size(11.0).color(muted()))
}

/// A fixed-height vertical spacer between demo blocks.
fn gap(h: f64) -> FlexChild<PlaygroundState> {
    inflexible(SizedBox(None, Some(h)))
}

/// A fixed-width horizontal spacer between chips in a row — [`gap`]'s
/// horizontal twin, needed by the stress-toggle button row.
fn gap_h(w: f64) -> FlexChild<PlaygroundState> {
    inflexible(SizedBox(Some(w), None))
}

/// Wrap a demo's rows in a padded vertical column (one showcase block).
fn block(children: Vec<FlexChild<PlaygroundState>>) -> FlexChild<PlaygroundState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

/// A bounded list of filler rows — the scrollable content that lets the Mode
/// B slot scroll fully off-screen and back.
/// Duplicated locally (private, per-module, not shared): 64 rows,
/// deliberately — at ~31 logical px per row this is
/// ~2000px of content, comfortably taller than any phone viewport (the
/// device-viewport lesson: a shorter filler barely exceeded a tall device's
/// body height, so scrolling read as broken on-device while the same filler
/// scrolled generously in a shorter headless test viewport, masking the bug).
fn filler_rows() -> AnyView<PlaygroundState> {
    let rows: Vec<AnyView<PlaygroundState>> = (1..=64)
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
/// fringing probe, static so it doesn't cost a
/// frame request at rest). `bump` feeds the slot's `params_json` — the
/// updateParams stress toggle's payload.
fn mode_b_slot(bump: u32) -> AnyView<PlaygroundState> {
    let params = format!("{{\"bump\":{bump}}}");
    let slot = maybe_debug_fill(
        platform_view(DEMO_STREAM_VIEW_TYPE)
            .size(SLOT_W, SLOT_H)
            .params_json(params)
            .semantics_label("Demo native stream (Mode B slot)"),
    );

    let opaque_btn = button("Opaque", |_: &mut PlaygroundState| {})
        .style(ButtonStyle::Primary)
        .small();
    let alpha_btn = AnimatedOpacity(
        0.5,
        button("50% alpha", |_: &mut PlaygroundState| {})
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
fn second_slot() -> AnyView<PlaygroundState> {
    let slot = maybe_debug_fill(
        platform_view(DEMO_STREAM_VIEW_TYPE)
            .size(SECOND_SLOT_W, SECOND_SLOT_H)
            .params_json("{\"slot\":\"second\"}")
            .semantics_label("Second demo native stream slot"),
    );
    any(slot)
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/// See the page-fn contract in [`crate::pages`]. See the [module docs](self)
/// for the full section breakdown.
pub fn page(_state: &PlaygroundState) -> AnyView<PlaygroundState> {
    let slot_visible = slot_visible_sig().get();
    let bump = params_bump_sig().get();
    let second_visible = second_slot_visible_sig().get();

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
                    |_: &mut PlaygroundState| {
                        let sig = slot_visible_sig();
                        sig.set(!sig.get_untracked());
                    },
                )
                .style(ButtonStyle::Secondary)
                .small())),
                gap_h(8.0),
                inflexible(any(button(
                    format!("Bump params ({bump})"),
                    |_: &mut PlaygroundState| {
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
                    |_: &mut PlaygroundState| {
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
             counter keeps moving (device-verified).",
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
    children.push(proof_strip);
    children.push(gap(12.0));
    children.push(inflexible(filler_rows()));

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, children),
    ))
}
