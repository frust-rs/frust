//! Interactions section: all eight moments
//! tied to actual muxr events — a connection heartbeat, a session-attach card
//! morph, a new-session boot sequence, a token-revoke character scramble, a
//! long-press charge ring, a pull-to-refresh rain burst, a live-output
//! waveform, and a copy-to-clipboard burst. The heartbeat/session-attach/
//! boot/waveform/clipboard-burst moments landed first (composition-only, no
//! new primitives); the scramble/charge-ring/rain-burst moments (this
//! addendum) lean on
//! `GestureDetector::on_hold_progress`/`hold_threshold_ms` for the charge ring
//! and `ScrollView::on_refresh_release` for the rain burst.
//!
//! # No new framework primitives
//!
//! Every moment composes existing facade widgets only. Two deliberate
//! substitutions from the original design's suggested mapping:
//!
//! - **Session attach** uses [`SharedAxis::Scaled`] instead of
//!   `TransitionPattern::ContainerTransform`: `ContainerTransform::new`/
//!   `from_source` takes a `Rect` (reachable, like every geometry type this
//!   file needs, through `frust::authoring`) this page has no other use for —
//!   `SharedAxis::Scaled` (incoming 0.80→1.0 scale + fade-through, no `Rect`
//!   needed) reads as the same "grows to fill" morph without pulling that type
//!   in for one call site — a design choice kept as-is by this file's facade
//!   migration, not a dependency constraint.
//! - **Per-frame demo state (heartbeat/boot/waveform/clipboard-burst) reads
//!   a wall clock, not a
//!   timer signal.** `frust::spawn_local` + `tokio::time::sleep` is the
//!   framework's blessed interval idiom (`examples/huddle`'s
//!   `clean_signals::time::sleep` precedent), but that sleep helper lives in
//!   the (unavailable here) `clean-signals` crate, and this crate has no
//!   direct `tokio` dependency either. Instead, each self-driving demo reads
//!   `std::time::Instant::now()` directly inside its page fn (permitted: the
//!   "no wall clock" rule in `docs/CODE_STANDARDS.md`'s Theming & Animation
//!   Conventions binds `frust-core`/`frust-widgets`, not application code —
//!   `examples/huddle` already reads `Instant` at this tier) and mounts a
//!   tiny [`FrameTicker`] — a hand-rolled
//!   `View`/`Widget` pair reached entirely through `frust::authoring`
//!   (mirrors `appbar.rs`'s `AnchorReporter`) whose only job is an
//!   unconditional `PaintCtx::request_frame` call in its own `paint` — to
//!   keep this page's `build` re-invoked every frame while an animation needs
//!   to keep advancing. **This replaces an earlier `pump()`**
//!   (a since-fixed bug): an invisible
//!   `AnimatedOpacity(0.0)` wrapping an Indeterminate `circular_progress`,
//!   riding *that* unrelated widget's own always-request-frame paint
//!   behavior as a side effect instead of declaring the need directly.
//!   Mounting [`frame_ticker`] is the same per-frame-rebuild idiom
//!   `frust_bench`'s `s6_text` `WidthPulse` pattern uses, adapted to a
//!   page fn with no `Component`/`PaintCtx` of its own — now via a widget
//!   that says what it's doing instead of exploiting one that doesn't.
//!
//! This addendum adds three more moments, one further substitution and two
//! implementation choices:
//!
//! - **The scramble moment's noise source is a tiny LCG** ([`lcg_next`]), not
//!   `Math.random()` — deterministic per `(frame, char index)` so the same
//!   hold/frame always renders the same glitch text (reproducible in a test,
//!   unlike a true RNG); no `Math.random` analog is needed here.
//! - **The charge-ring moment's "shadow" is an amber wash layer, not
//!   `PaintScene::draw_shadow`.**
//!   That call is a `PaintCtx`/`Widget::paint` primitive with no facade
//!   equivalent reachable from application code (every demo *view* in this
//!   file composes existing facade widgets only — the sole exception is
//!   [`FrameTicker`] above, a narrow, documented escape hatch, not a general
//!   license to hand-roll widgets) — an `AnimatedOpacity`-faded [`Image`] wash
//!   behind the row (the same `solid_source` technique
//!   [`demo_copy_burst`]'s flash uses) reads as a comparable "lift" cue
//!   without it.
//! - **The rain moment's rain is per-frame positioned glyph views on a
//!   fixed column grid** — an honest-composition route, not a `draw_shader`
//!   WGSL quad — consistent with every other demo on this page staying
//!   inside the plain widget-composition surface (no `frust_scene`/shader
//!   dependency to add to this crate's manifest, mirroring the
//!   session-attach substitution's deps-unchanged constraint above). The
//!   column count is fixed rather than measured-width-derived (application
//!   code has no layout-pass access to its own resolved size), so the
//!   refresh zone itself is a fixed-width
//!   [`SizedBox`] ([`RAIN_ZONE_W`]) rather than filling the available width.
//!
//! # Reduced motion
//!
//! Every clock-driven demo checks a local `reduce` flag directly —
//! `state.reduce_motion.get() || !state.animations_enabled.get()`, ORing the
//! header animations-off toggle into the same check rather than adding a
//! second gating flag (see
//! `crate::CatalogState::animations_enabled`'s doc comment) — because the
//! `pattern_switcher`/`AnimatedOpacity`/`AnimatedScale` wrappers only
//! auto-collapse when resolving a *theme-default* timing, which these
//! wall-clock-driven demos deliberately bypass via an explicit
//! `Timing::Duration(Duration::ZERO, ..)` "immediate follower" — see [`ZERO`]
//! and skips the animated portion outright rather than fighting the built-in
//! collapse: the heartbeat ring/latency-roll, waveform bars, and copy-burst
//! particles all render their settled end-state instead, matching
//! `motion.rs`'s reduced-motion note ("every demo below collapses"). The
//! scramble/rain moments (also wall-clock-driven) follow the same rule — a
//! revoke jumps straight to the collapsed row, a refresh straight to "last
//! updated just now", no scramble/rain frames rendered. The charge-ring
//! moment is the one exception on this page: its
//! progress comes from `GestureDetectorView::on_hold_progress`, an
//! event-delivered (not wall-clock) observation with no theme-default timing
//! to bypass, so it needs no `ZERO`-timing workaround — reduced motion there
//! only drops the ring/scale visuals, not the underlying gesture contract.
//!
//! # Tap-to-play
//!
//! The heartbeat moment's ping/rolling-latency was originally the page's one
//! self-driving exception — it auto-repeated forever with no input, the one
//! documented exception to "every demo starts idle". This page removes that
//! exception: combined with the scroll view's visible-rect paint culling,
//! nothing on this page should burn frames while unseen, and an
//! always-running demo defeats that even while it IS seen but nobody's
//! looking at it. `hb_active_sig` (mirroring every other demo's own
//! `*_active_sig`) now defaults `false` — stopped, rendering the settled
//! first latency sample with the ping ring hidden, no [`frame_ticker`]
//! mounted — and a [`GestureDetector`] wrapping the row (this page's "tap
//! the demo card" affordance; a card-shaped tap target reads more naturally
//! here than a button, unlike the waveform moment's existing `Toggle
//! build-watch activity` button, which already satisfied the same play/stop
//! contract and is left as-is) starts/stops the wall-clock loop on tap,
//! resetting the clock on each (re)start so a cycle always begins at the
//! ping. The boot demo and the scramble/charge-ring/rain/clipboard-burst
//! demos were already tap-triggered and self-stopping and need no change
//! here.
//!
//! Structure mirrors `motion.rs`: `pub fn page(state)` + one `demo_*` fn per
//! moment inside `block(...)` scaffolds, thread-local demo state via the same
//! `local_sig!` macro.

use std::time::{Duration, Instant};

// FrameTicker (see the module docs' [`FrameTicker`] note) is a hand-rolled
// `View`/`Widget` pair — reached, like every other widget in this file,
// entirely through the `frust` facade. Mirrors `appbar.rs`'s `AnchorReporter`.
use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::motion::patterns::SharedAxis;
use frust::motion::switcher::pattern_switcher;
use frust::motion::{AnimatedOpacity, AnimatedScale};
use frust::{
    Align, Alignment, AnyView, Axis, ButtonStyle, Color, CrossAxisAlignment, Curve, EdgeInsets,
    FlexChild, FlexView, GestureDetector, Get, GetUntracked, Image, ImageFit, ImageSource, Padding,
    RwSignal, Set, SizedBox, Stack, Theme, Timing, Update, any, button, flexible, inflexible,
    keyed, scroll_view, text, use_context,
};
use frust_glyph::{BadgeVariant, TermLine, badge, glyph_card, term_block};
// `circular_progress`/`ProgressValue` are Material catalog items (see
// `Cargo.toml`'s `frust-material` dependency comment) — not
// `frust_glyph::*`, but not baseline `frust`/`frust-widgets` items either.
use frust_material::{ProgressValue, circular_progress};

use crate::CatalogState;

/// Glyph accent amber — see `motion.rs`'s twin (each page file resolves its
/// own live-theme roles; not shared across files, matching
/// `navigation.rs`/`overlays.rs`'s existing per-file precedent).
fn amber() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .primary
}

/// A muted caption ink — see [`amber`]'s twin.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .on_surface_variant
}

/// The error/danger ink — [`demo_token_scramble`]'s glitching-token tint. See
/// [`amber`]'s twin.
fn error_ink() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .error
}

/// `color` with its alpha channel replaced — duplicated from
/// `frust-glyph::badge`'s crate-private helper of the same shape
/// (unreachable from here), used for the copy-field's flash wash.
fn with_alpha(color: Color, alpha: f32) -> Color {
    let c = color.components;
    Color::new([c[0], c[1], c[2], alpha])
}

/// A 1×1 solid-color [`ImageSource`] — the facade-only way to paint an
/// arbitrary filled rectangle (`foundations.rs`'s documented technique,
/// reproduced locally per that file's own note that this crate has no
/// generic "filled box" widget).
fn solid_source(color: Color) -> ImageSource {
    let c = color.components;
    let to_u8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    ImageSource::from_rgba8(
        vec![to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), to_u8(c[3])],
        1,
        1,
    )
}

/// An immediate "current value" follower timing — zero duration, so an
/// [`AnimatedOpacity`]/[`AnimatedScale`] wrapper just snaps to whatever target
/// its caller computed this frame (from a wall-clock read) rather than
/// re-easing on top of it. See the [module docs](self)'s reduced-motion note
/// for why this deliberately bypasses the wrappers' theme-default (and thus
/// `reduce_motion`-blind) spring.
const ZERO: Timing = Timing::Duration(Duration::ZERO, Curve::Linear);

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!`, self-healing across a disposed owner —
/// duplicated from `motion.rs`'s macro of the same shape/rationale (private,
/// not exported across page modules).
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

/// A wall-clock stopwatch cached in a `thread_local!`, reset by calling
/// `$reset_name()` — the elapsed-time source every clock-driven demo below
/// reads from (see the [module docs](self)'s wall-clock note). The
/// `thread_local!` lives in a private module scoped to this invocation (named
/// after `$elapsed_name`, which shares no namespace with the sibling `fn` of
/// the same name — modules and functions occupy different Rust namespaces) so
/// `$elapsed_name`/`$reset_name` share exactly one `START` cell rather than
/// each function macro-expanding its own separate static.
macro_rules! local_clock {
    ($elapsed_name:ident, $reset_name:ident) => {
        mod $elapsed_name {
            use std::cell::Cell;
            use std::time::Instant;
            thread_local! {
                pub(super) static START: Cell<Option<Instant>> = const { Cell::new(None) };
            }
        }
        // Some clocks (the heartbeat/waveform ones) are deliberately
        // free-running and never reset — `#[allow(dead_code)]` covers that
        // case without warning under the workspace's `-D warnings` gate.
        #[allow(dead_code)]
        fn $reset_name() {
            $elapsed_name::START.with(|c| c.set(Some(Instant::now())));
        }
        fn $elapsed_name() -> f64 {
            let now = Instant::now();
            let start = $elapsed_name::START.with(|c| {
                if let Some(s) = c.get() {
                    s
                } else {
                    c.set(Some(now));
                    now
                }
            });
            now.duration_since(start).as_secs_f64() * 1000.0
        }
    };
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

/// Wrap a demo's rows in a padded vertical column (one showcase card) —
/// mirrors `motion.rs`'s helper of the same name/shape.
fn block(children: Vec<FlexChild<CatalogState>>) -> FlexChild<CatalogState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

// ---------------------------------------------------------------------------
// FrameTicker — the pump() replacement (see the module docs' wall-clock note)
// ---------------------------------------------------------------------------

/// A zero-size sentinel [`View`]/[`Widget`] pair whose only job is an
/// unconditional [`PaintCtx::request_frame`] call in its own `paint` —
/// mounted by a `demo_*` fn only while its own wall-clock animation is still
/// running, so a request traces directly to the demo that issued it (see the
/// [module docs](self)'s wall-clock note). Deliberately does not reuse
/// `circular_progress(ProgressValue::Indeterminate)`'s own always-request
/// paint behavior the way the deleted `pump()` did — that made the request
/// an unrelated side effect instead of this widget's stated purpose.
struct FrameTicker;

impl<State: 'static> View<State> for FrameTicker {
    type Element = FrameTickerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> FrameTickerWidget {
        FrameTickerWidget
    }

    fn rebuild(
        &self,
        _prev: &Self,
        _element: &mut FrameTickerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        ChangeFlags::PAINT
    }
}

/// The retained widget for a [`FrameTicker`]. See its doc comment.
struct FrameTickerWidget;

impl Widget for FrameTickerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame();
    }
}

/// Mount a [`FrameTicker`] — call only while the caller's own wall-clock demo
/// still needs another frame; the widget itself just asks. Mirrors the
/// deleted `pump()`'s mount discipline: the call site (not the widget)
/// decides when a frame is actually needed.
fn frame_ticker() -> FlexChild<CatalogState> {
    inflexible(FrameTicker)
}

// ---------------------------------------------------------------------------
// 01 — connection heartbeat + rolling latency
// ---------------------------------------------------------------------------

local_sig!(hb_active_sig, bool, false);
local_clock!(hb_elapsed_ms, hb_reset_clock);

/// Full cycle length: the ping ring fires, then latency rolls, then idles
/// until the next cycle — matching the reference's "runs automatically every
/// 3s" caption, now gated behind [`hb_active_sig`] (see this module's
/// "Tap-to-play" docs above).
const HB_CYCLE_MS: f64 = 3000.0;
/// Ping-ring animation length (reference: 1.6s `pingRing` keyframe).
const HB_RING_MS: f64 = 1600.0;
/// Latency-roll animation length (reference: 10 steps × 30ms).
const HB_ROLL_MS: f64 = 300.0;
/// A small fixed cycle of "looks alive" latency samples — deterministic
/// rather than a `rand` dependency this crate doesn't have (Acceptance
/// Criteria #2: catalog deps unchanged).
const HB_LATENCIES: [f64; 6] = [36.0, 52.0, 41.0, 68.0, 29.0, 44.0];

/// Ping-ring [`AnimatedScale`] range: starts subtly inset (`0.6×` — a small
/// "closed" pip) and grows well past the icon slot (`2.8×`) as it fades, the
/// radar-expand feel the reference keyframe animates.
const HB_RING_SCALE_MIN: f64 = 0.6;
const HB_RING_SCALE_MAX: f64 = 2.8;

/// The ping ring's own (unscaled) diameter — the box [`circular_progress`]
/// strokes its full circle into, matching the original `SizedBox(18, 18)`.
const HB_RING_DIAMETER: f64 = 18.0;

/// Paint headroom for the ping ring: [`AnimatedOpacity`] composites its child
/// under
/// [`PaintScene::push_layer`], which records ITS OWN (unscaled) layout rect
/// as the layer's clip — captured *before* a nested [`AnimatedScale`] has
/// pushed its scale transform, since `AnimatedOpacity` paints outermost. A
/// `SizedBox(HB_RING_DIAMETER, HB_RING_DIAMETER)` slot therefore clips the
/// ring to an 18×18 square the instant it scales past 1.0× — confirmed by
/// this module's `heartbeat_ring_stays_within_its_headroom_slot_at_max_extent`
/// recording-fake test, which found the cut is a real `push_layer` clip
/// rect, not merely a layout-allocation illusion. The fix widens
/// the *outer* slot the `AnimatedOpacity` itself lays out at (this constant,
/// `>= HB_RING_DIAMETER * HB_RING_SCALE_MAX` — `18.0 * 2.8 = 50.4`, plus a
/// small rounding margin) and centers the small unscaled ring inside it via
/// [`Align`], so the recorded clip rect is already big enough to contain the
/// circle at every scale up to [`HB_RING_SCALE_MAX`] — nothing left to clip.
const HB_RING_SLOT: f64 = 52.0;

/// Pure ping-ring geometry at `elapsed_in_cycle` ms into [`demo_heartbeat`]'s
/// per-cycle clock (`reduce` suppresses the ping outright, matching
/// `demo_heartbeat`'s own reduced-motion branch) — split out from
/// `demo_heartbeat` so this module's effect-slot-headroom tests can
/// drive exact start/mid/max-extent timestamps directly instead of depending
/// on [`hb_elapsed_ms`]'s wall clock. Returns `(ring_opacity, ring_scale)`.
fn heartbeat_ring_state(elapsed_in_cycle: f64, reduce: bool) -> (f64, f64) {
    let ring_active = !reduce && elapsed_in_cycle < HB_RING_MS;
    let ring_p = if ring_active {
        (elapsed_in_cycle / HB_RING_MS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let ring_opacity = if ring_active {
        0.7 * (1.0 - ring_p)
    } else {
        0.0
    };
    let ring_scale = HB_RING_SCALE_MIN + (HB_RING_SCALE_MAX - HB_RING_SCALE_MIN) * ring_p;
    (ring_opacity, ring_scale)
}

/// Build the ping ring's view from its already-computed `(opacity, scale)`
/// (see [`heartbeat_ring_state`]) — a [`HB_RING_SLOT`]-sized headroom
/// [`SizedBox`], [`Align`]-centering the actual [`HB_RING_DIAMETER`] ring
/// content, all under the same `AnimatedOpacity`-outside/`AnimatedScale`-inside
/// nesting as before (see [`HB_RING_SLOT`]'s doc comment for why the nesting
/// order — not just the sizing — matters here).
fn heartbeat_ring_view(ring_opacity: f64, ring_scale: f64) -> AnyView<CatalogState> {
    any(AnimatedOpacity(
        ring_opacity,
        SizedBox(Some(HB_RING_SLOT), Some(HB_RING_SLOT)).child(Align(
            Alignment::CENTER,
            AnimatedScale(
                ring_scale,
                SizedBox(Some(HB_RING_DIAMETER), Some(HB_RING_DIAMETER))
                    .child(circular_progress(ProgressValue::Determinate(1.0))),
            )
            .timing(ZERO),
        )),
    )
    .timing(ZERO))
}

/// 01 connection heartbeat: a radar-ping ring (a full-circle
/// [`circular_progress`] stroke under [`AnimatedOpacity`]/[`AnimatedScale`],
/// driven by a wall-clock read — no per-widget custom paint needed) plus a
/// rolling (not snapping) latency readout. Tap-to-play: stopped by default,
/// rendering the settled first latency sample
/// with the ring hidden; tapping the row starts the wall-clock loop, and
/// tapping again (or the header's animations-off toggle) stops it.
fn demo_heartbeat(state: &CatalogState) -> FlexChild<CatalogState> {
    // ORs the header animations-off toggle into the same local `reduce`
    // check every wall-clock demo already gates on — the "cheapest sound
    // wiring" `CatalogState::animations_enabled`'s doc comment describes,
    // not a second flag.
    let reduce = state.reduce_motion.get() || !state.animations_enabled.get();
    let active_sig = hb_active_sig();
    // The global animations-off toggle force-stops this demo like every
    // other one on the page even while `hb_active_sig` itself is still
    // `true` underneath — flipping animations back on resumes the same
    // never-reset wall clock rather than restarting the cycle (see this
    // module's "Tap-to-play" docs above).
    let active = active_sig.get() && !reduce;

    let (t, cycle) = if active {
        let elapsed = hb_elapsed_ms();
        (
            elapsed % HB_CYCLE_MS,
            (elapsed / HB_CYCLE_MS).floor().max(0.0) as usize,
        )
    } else {
        (0.0, 0)
    };

    let from_latency = HB_LATENCIES[cycle % HB_LATENCIES.len()];
    let to_latency = HB_LATENCIES[(cycle + 1) % HB_LATENCIES.len()];
    let rolling = active && t < HB_ROLL_MS;
    let latency = if !active {
        // Stopped: the settled "nothing has happened yet" reading — a
        // representative settled frame rather than a blank/zero value.
        HB_LATENCIES[0]
    } else if rolling {
        from_latency + (to_latency - from_latency) * (t / HB_ROLL_MS)
    } else {
        to_latency
    };

    // Stopped renders the ring fully transparent at its MIN (not MAX) scale
    // — deliberately not `heartbeat_ring_state(t, true)`'s own settled read
    // (opacity 0 / scale `HB_RING_SCALE_MAX`), which this module's own
    // `heartbeat_ping_headroom_does_not_stretch_sibling_row_content` test
    // caught corrupting this row's *sibling* (post-ring) layout the instant
    // `reduce_motion` forces that exact (0.0, HB_RING_SCALE_MAX) pair — a
    // pre-existing `AnimatedScale`/`Flex` interaction defect outside the
    // scope of this file (`examples/glyph-catalog/src/pages/interactions.rs`
    // only), reproduced identically against the code as it existed before
    // the tap-to-play rework by forcing `reduce_motion` on that same test.
    // `HB_RING_SCALE_MIN` (the ring's cycle-start pose) renders identically
    // invisible at `ring_opacity == 0.0` and avoids the defect entirely.
    let (ring_opacity, ring_scale) = if active {
        heartbeat_ring_state(t, false)
    } else {
        (0.0, HB_RING_SCALE_MIN)
    };
    let ring = heartbeat_ring_view(ring_opacity, ring_scale);

    let row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(ring),
            inflexible(SizedBox(Some(6.0), None)),
            inflexible(badge("connected", BadgeVariant::Success).dot(true)),
            inflexible(SizedBox(Some(10.0), None)),
            flexible(1, text("100.71.31.57:50051 · zellij 0.44.3").size(11.5)),
            inflexible(
                text(format!("{:.0}ms", latency))
                    .size(12.0)
                    .color(if rolling { amber() } else { muted() }),
            ),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center);

    // The tap-to-play affordance (see this module's "Tap-to-play" docs
    // above) — tapping the row toggles `hb_active_sig`, resetting the wall
    // clock on each (re)start so a cycle always begins at the ping rather
    // than resuming mid-cycle.
    let tappable_row: AnyView<CatalogState> =
        any(GestureDetector(row).on_tap(move |_s: &mut CatalogState| {
            if active_sig.get_untracked() {
                active_sig.set(false);
            } else {
                hb_reset_clock();
                active_sig.set(true);
            }
        }));

    let mut children = vec![
        inflexible(label("01 Connection heartbeat")),
        inflexible(caption(if reduce {
            "reduced motion — ring suppressed; latency updates instantly"
        } else if active {
            "tap to stop — radar ping ring pulses every 3s; latency rolls rather than snapping"
        } else {
            "tap the row to start the connection heartbeat"
        })),
        gap(6.0),
        inflexible(tappable_row),
    ];
    if active {
        children.push(frame_ticker());
    }
    block(children)
}

// ---------------------------------------------------------------------------
// 02 — session attach (card → terminal morph)
// ---------------------------------------------------------------------------

local_sig!(attach_sig, bool, false);

/// 02 session attach: tapping the compact card morphs it into the full
/// terminal face (`SharedAxis::Scaled` — see the [module docs](self)'s
/// substitution note); the back link reverses it.
fn demo_attach() -> FlexChild<CatalogState> {
    let attached_sig = attach_sig();
    let attached = attached_sig.get();
    let key = usize::from(attached);

    let face: AnyView<CatalogState> = if attached {
        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(
                    FlexView::new(
                        Axis::Horizontal,
                        vec![
                            inflexible(
                                button("‹ sessions", move |_s: &mut CatalogState| {
                                    attached_sig.set(false)
                                })
                                .style(ButtonStyle::Ghost)
                                .small(),
                            ),
                            flexible(1, text("dev").size(12.0).color(amber())),
                        ],
                    )
                    .cross_axis(CrossAxisAlignment::Center),
                ),
                gap(8.0),
                inflexible(term_block(vec![
                    TermLine::prompt("ed@dev:~$ btop"),
                    TermLine::output("cpu 12% · mem 4.1/16gb"),
                ])),
            ],
        ))
    } else {
        any(GestureDetector(
            glyph_card::<CatalogState>()
                .title(text("dev").size(13.0))
                .desc(text("9 panes · 4 tabs · running").size(11.0).color(muted())),
        )
        .on_tap(move |_s: &mut CatalogState| attached_sig.set(true)))
    };

    block(vec![
        inflexible(label("02 Session attach")),
        inflexible(caption(
            "tap the card — it grows to fill the viewport and becomes the terminal chrome",
        )),
        gap(6.0),
        inflexible(pattern_switcher(key, SharedAxis::Scaled, face)),
    ])
}

// ---------------------------------------------------------------------------
// 03 — new session boot
// ---------------------------------------------------------------------------

local_sig!(boot_active_sig, bool, false);
local_sig!(boot_replay_sig, usize, 0);
local_clock!(boot_elapsed_ms, boot_reset_clock);

/// Delay before the final "running" chip fades in, after the boot output —
/// roughly the reference's 3-line stagger (`term_block`'s fixed ~90ms/line,
/// see `motion.rs`'s `demo_boot` note) plus a settle beat.
const BOOT_CHIP_DELAY_MS: f64 = 420.0;

fn boot_lines() -> Vec<TermLine> {
    vec![
        TermLine::prompt("muxrd spawn --shell zsh"),
        TermLine::output("allocating pty…"),
        TermLine::output("attaching zellij session \"swift-otter\"…"),
    ]
}

/// 03 new session boot: the "+ new session" button morphs into a card
/// (`SharedAxis::Scaled`, mirroring [`demo_attach`]) showing real-looking
/// staggered boot output (`term_block(...).staggered(true)` — the same
/// primitive `motion.rs`'s `demo_boot` uses), then a final status chip fades
/// in once the lines have had time to reveal.
fn demo_boot() -> FlexChild<CatalogState> {
    let active_sig = boot_active_sig();
    let replay_sig = boot_replay_sig();
    let active = active_sig.get();
    let n = replay_sig.get();
    let key = usize::from(active);

    let frame: AnyView<CatalogState> = if active {
        let elapsed = boot_elapsed_ms();
        let chip_visible = elapsed >= BOOT_CHIP_DELAY_MS;
        let chip_row = FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(badge("running", BadgeVariant::Success).dot(true)),
                inflexible(SizedBox(Some(8.0), None)),
                inflexible(text("swift-otter").size(12.0)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center);

        let mut card_children = vec![
            keyed(n, term_block(boot_lines()).staggered(true)),
            gap(8.0),
            inflexible(AnimatedOpacity(
                if chip_visible { 1.0 } else { 0.0 },
                chip_row,
            )),
        ];
        if !chip_visible {
            card_children.push(frame_ticker());
        }
        any(glyph_card::<CatalogState>().desc(any(FlexView::new(Axis::Vertical, card_children))))
    } else {
        any(GestureDetector(
            glyph_card::<CatalogState>().desc(text("+ new session").size(12.0).color(amber())),
        )
        .on_tap(move |_s: &mut CatalogState| {
            boot_reset_clock();
            replay_sig.update(|c| *c += 1);
            active_sig.set(true);
        }))
    };

    block(vec![
        inflexible(label("03 New session boot")),
        inflexible(caption(
            "button morphs into a card of staggered boot output, then a running chip",
        )),
        gap(6.0),
        inflexible(pattern_switcher(key, SharedAxis::Scaled, frame)),
        gap(6.0),
        inflexible(
            button("Replay", move |_s: &mut CatalogState| {
                boot_reset_clock();
                replay_sig.update(|c| *c += 1);
                active_sig.set(true);
            })
            .small(),
        ),
    ])
}

// ---------------------------------------------------------------------------
// Shared PRNG (used by §04/§06)
// ---------------------------------------------------------------------------

/// One step of a small linear-congruential generator (Numerical Recipes'
/// constants) — the deterministic "no `Math.random` analog needed" noise
/// source [`scramble_char`]/[`rain_char`] use. Given the same `seed` this
/// always returns the same value, so a given `(frame, index)` pair always
/// renders the same glitch/rain character — reproducible, not a true RNG.
fn lcg_next(seed: u32) -> u32 {
    seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

// ---------------------------------------------------------------------------
// 04 — token revoke scramble
// ---------------------------------------------------------------------------

local_sig!(token_active_sig, bool, false);
local_clock!(token_elapsed_ms, token_reset_clock);

/// The demo token, and the noise charset — both lifted verbatim from the
/// reference's `ORIGINAL_TOKEN`/`CHARS`.
const SCRAMBLE_TOKEN: &str = "zelli-mobile-100-71-31-57-mr3g6w5g";
const SCRAMBLE_CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-#$%&";
/// Per-frame tick length (reference: `setInterval(..., 40)`).
const SCRAMBLE_FRAME_MS: f64 = 40.0;
/// Total scramble frames before the string freezes (reference: `maxFrames`).
const SCRAMBLE_MAX_FRAMES: f64 = 14.0;
/// Delay after the last scramble frame before the collapse starts (reference:
/// the `setTimeout(..., 120)` that adds `.collapsing`).
const SCRAMBLE_COLLAPSE_DELAY_MS: f64 = 120.0;
/// Collapse (max-height/opacity) transition length (reference: `.3s`).
const SCRAMBLE_COLLAPSE_MS: f64 = 300.0;
/// The row's at-rest height, in logical px (this demo's own choice — the
/// reference's `max-height:60px` is a CSS cap, not the rendered height).
const SCRAMBLE_ROW_H: f64 = 40.0;

/// One noise character for token index `idx` at scramble `frame` — mirrors
/// `Math.random()<0.5 ? CHARS[...] : original` with [`lcg_next`] standing in
/// for `Math.random`. A space always stays a space (matches the reference's
/// `ORIGINAL_TOKEN[i]==' '?' '` guard, though this token has none).
fn scramble_char(original: char, frame: u32, idx: u32) -> char {
    if original == ' ' {
        return ' ';
    }
    let r1 = lcg_next(frame.wrapping_mul(0x9E37_79B1).wrapping_add(idx));
    if r1 & 1 == 0 {
        original
    } else {
        let r2 = lcg_next(r1);
        SCRAMBLE_CHARSET[(r2 as usize) % SCRAMBLE_CHARSET.len()] as char
    }
}

/// The full scrambled string at `frame` (0..=14): the left `lock_ratio*40%`
/// of characters are locked to `·`, the rest noisy — mirrors the reference's
/// per-character loop. The frame's one allocation is the rebuilt string
/// itself — nothing else allocates per frame.
fn scramble_text(frame: u32) -> String {
    let lock_ratio = frame as f64 / SCRAMBLE_MAX_FRAMES;
    let len = SCRAMBLE_TOKEN.chars().count();
    SCRAMBLE_TOKEN
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            if len > 0 && (i as f64 / len as f64) < lock_ratio * 0.4 {
                if ch == ' ' { ' ' } else { '·' }
            } else {
                scramble_char(ch, frame, i as u32)
            }
        })
        .collect()
}

/// The token name text, tinted [`error_ink`] while glitching.
fn token_name_view(rendered: String, glitching: bool) -> AnyView<CatalogState> {
    let t = text(rendered).size(11.5);
    if glitching {
        any(t.color(error_ink()))
    } else {
        any(t)
    }
}

/// 04 token revoke — scramble: revoking progressively locks the token's
/// characters left-to-right through noise (14 ticks × 40ms, [`scramble_text`]),
/// then the row collapses (height + opacity, driven by the same wall clock —
/// no `request_layout` needed since a per-frame `SizedBox` height change
/// already marks `ChangeFlags::LAYOUT` on rebuild). "Restore row" resets.
fn demo_token_scramble(state: &CatalogState) -> FlexChild<CatalogState> {
    // ORs the header animations-off toggle into the same local `reduce`
    // check every wall-clock demo already gates on — the "cheapest sound
    // wiring" `CatalogState::animations_enabled`'s doc comment describes,
    // not a second flag.
    let reduce = state.reduce_motion.get() || !state.animations_enabled.get();
    let active_sig = token_active_sig();
    let active = active_sig.get();

    // (display text, glitch tint, height/opacity ratio, still animating?)
    let (display, glitching, ratio, running) = if !active {
        (SCRAMBLE_TOKEN.to_string(), false, 1.0, false)
    } else if reduce {
        // Reduced motion: settle straight to the collapsed end-state, no
        // scramble frames rendered (module docs' Reduced motion note).
        (String::new(), false, 0.0, false)
    } else {
        let elapsed = token_elapsed_ms();
        let frame = (elapsed / SCRAMBLE_FRAME_MS)
            .floor()
            .clamp(0.0, SCRAMBLE_MAX_FRAMES) as u32;
        let collapse_start = SCRAMBLE_MAX_FRAMES * SCRAMBLE_FRAME_MS + SCRAMBLE_COLLAPSE_DELAY_MS;
        let collapse_p = if elapsed > collapse_start {
            ((elapsed - collapse_start) / SCRAMBLE_COLLAPSE_MS).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (
            scramble_text(frame),
            true,
            1.0 - collapse_p,
            collapse_p < 1.0,
        )
    };

    let row_inner = FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(1, token_name_view(display, glitching)),
            inflexible(SizedBox(Some(8.0), None)),
            inflexible(
                button("revoke", move |_s: &mut CatalogState| {
                    token_reset_clock();
                    active_sig.set(true);
                })
                .style(ButtonStyle::Danger)
                .small(),
            ),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center);

    let row: AnyView<CatalogState> = any(AnimatedOpacity(
        ratio,
        SizedBox(None, Some((SCRAMBLE_ROW_H * ratio).max(0.0)))
            .child(Padding(EdgeInsets::symmetric(4.0, 6.0), row_inner)),
    )
    .timing(ZERO));

    let mut children = vec![
        inflexible(label("04 Token revoke — scramble")),
        inflexible(caption(if reduce {
            "reduced motion — revoke collapses the row instantly, no scramble"
        } else {
            "revoke — characters lock left-to-right through noise, then the row collapses"
        })),
        gap(6.0),
        inflexible(row),
        gap(6.0),
        inflexible(
            button("Restore row", move |_s: &mut CatalogState| {
                active_sig.set(false);
            })
            .small(),
        ),
    ];
    if running {
        children.push(frame_ticker());
    }
    block(children)
}

// ---------------------------------------------------------------------------
// 05 — long-press charge ring → float
// ---------------------------------------------------------------------------

local_sig!(charge_progress_sig, f64, 0.0);
local_sig!(charge_floated_sig, bool, false);

/// Hold duration before the row floats (reference: "~700ms").
const CHARGE_HOLD_MS: u64 = 700;
/// The leading icon box's side length, in logical px.
const CHARGE_ICON_SIZE: f64 = 26.0;
/// The charge ring's diameter — slightly larger than the icon so the arc
/// traces around it, matching the reference's ring-around-the-icon framing
/// (a static position here rather than around the pointer: application code
/// has no reach to the raw pointer coordinates `on_hold_progress` observes).
const CHARGE_RING_SIZE: f64 = 34.0;
/// The row's fixed height, in logical px.
const CHARGE_ROW_H: f64 = 56.0;
/// The float glow/shadow wash's alpha: bumped up from an earlier `0.10` for
/// contrast on the Glyph dark baseline (the shadow needs visible contrast on
/// the dark theme too) — matches [`demo_charge_ring`]'s own floated
/// `icon_bg` wash alpha (`0.18`) just below, a value already proven legible
/// on this page.
const CHARGE_GLOW_ALPHA: f32 = 0.18;

/// 05 long-press → float: holding the row past [`CHARGE_HOLD_MS`] fills a
/// charge ring ([`circular_progress`]'s stroked-arc sweep, the same primitive
/// the heartbeat ping ring uses) via
/// `GestureDetectorView::on_hold_progress`/`GestureDetectorView::hold_threshold_ms`;
/// reaching the threshold "floats" the row (scale 1.03 + an amber
/// wash standing in for the reference's box-shadow — see the [module
/// docs](self)'s substitution note — + a chip); an early lift or slop-break
/// resets the ring to empty (the widget's own final-`0.0` contract). Tapping
/// a floated row drops it back down.
///
/// **Reset idiom**: `on_hold_progress`'s Cancel staleness gap
/// (`frust_widgets::gesture`'s module docs) means a platform `Cancel`
/// mid-hold delivers no final observation, so a naive consumer could be left
/// showing a stale, frozen ring from an interrupted hold. This demo's
/// `on_hold_progress` handler always trusts the widget's latest observation
/// — never clamping it to be monotonically non-decreasing — so a fresh press
/// cycle's low-restarting first observation (`press_start` re-anchors on
/// every `Down`) overwrites any stale value immediately, letting the ring
/// self-correct the moment the *next* hold begins rather than only after a
/// full press-release cycle completes.
fn demo_charge_ring(state: &CatalogState) -> FlexChild<CatalogState> {
    // ORs the header animations-off toggle into the same local `reduce`
    // check every wall-clock demo already gates on — the "cheapest sound
    // wiring" `CatalogState::animations_enabled`'s doc comment describes,
    // not a second flag.
    let reduce = state.reduce_motion.get() || !state.animations_enabled.get();
    let progress_sig = charge_progress_sig();
    let floated_sig = charge_floated_sig();
    let progress = progress_sig.get();
    let floated = floated_sig.get();
    let holding = !floated && progress > 0.0;

    let icon_bg = if floated {
        with_alpha(amber(), 0.18)
    } else {
        with_alpha(muted(), 0.14)
    };
    let icon_fg = if floated { amber() } else { muted() };
    let icon_box: AnyView<CatalogState> =
        any(
            SizedBox(Some(CHARGE_ICON_SIZE), Some(CHARGE_ICON_SIZE)).child(Stack(vec![
                any(Image(solid_source(icon_bg)).fit(ImageFit::Fill)),
                any(Align(
                    Alignment::CENTER,
                    text("▤").size(12.0).color(icon_fg),
                )),
            ])),
        );

    let mut icon_layers = vec![icon_box];
    if holding && !reduce {
        icon_layers.push(any(Align(
            Alignment::CENTER,
            SizedBox(Some(CHARGE_RING_SIZE), Some(CHARGE_RING_SIZE))
                .child(circular_progress(ProgressValue::Determinate(progress))),
        )));
    }
    let icon_stack: AnyView<CatalogState> =
        any(SizedBox(Some(CHARGE_RING_SIZE), Some(CHARGE_RING_SIZE)).child(Stack(icon_layers)));

    let title_col: AnyView<CatalogState> = any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(text("(untitled)").size(11.5)),
            inflexible(text("/home/ed/dev/forgekit").size(9.5).color(muted())),
        ],
    ));

    let chip: AnyView<CatalogState> = any(AnimatedOpacity(
        if floated { 1.0 } else { 0.0 },
        badge("floating", BadgeVariant::Accent),
    )
    .timing(ZERO));

    let row_inner = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(icon_stack),
            inflexible(SizedBox(Some(10.0), None)),
            flexible(1, title_col),
            inflexible(chip),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center);

    // The reference's `box-shadow` lift has no facade equivalent from
    // application code (see the module docs' substitution note) — an
    // opacity-faded amber wash behind the row approximates it.
    //
    // Effect-slot-headroom fix: a bare `SizedBox(None, Some(CHARGE_ROW_H))`
    // leaves its WIDTH axis unconstrained, so under this `Stack` layer's
    // loosened-but-bounded constraint (`min` relaxed to `0`, `max` still the
    // row's real width) `Image`'s layout falls back to its *natural* size —
    // the 1×1 solid source — clamped to `(1px, CHARGE_ROW_H)`: a vertical
    // hairline, not a row-wide wash (confirmed by this module's
    // `charge_ring_glow_fills_its_row_width` recording-fake test; NOT the
    // scale-clip mechanism first suspected — see that test's doc comment).
    // `Flex`'s own `flexible(1, ..)` share
    // computation is what actually forces a TIGHT (not just bounded) width
    // in this widget set with no dedicated "fill" primitive, so the fix
    // wraps the wash in a single-child horizontal `flexible(1, ..)` row:
    // its main axis (width) is tightened to the row's full available width
    // exactly the way a flexible flex child's main axis always is.
    let glow_wash: AnyView<CatalogState> = any(FlexView::new(
        Axis::Horizontal,
        vec![flexible(
            1,
            SizedBox(None, Some(CHARGE_ROW_H)).child(
                Image(solid_source(with_alpha(amber(), CHARGE_GLOW_ALPHA))).fit(ImageFit::Fill),
            ),
        )],
    ));
    let glow: AnyView<CatalogState> =
        any(AnimatedOpacity(if floated { 1.0 } else { 0.0 }, glow_wash).timing(ZERO));

    let card: AnyView<CatalogState> = any(SizedBox(None, Some(CHARGE_ROW_H)).child(Stack(vec![
        glow,
        any(Padding(EdgeInsets::all(12.0), row_inner)),
    ])));

    let scaled: AnyView<CatalogState> = if reduce {
        card
    } else {
        any(AnimatedScale(if floated { 1.03 } else { 1.0 }, card).timing(ZERO))
    };

    let gesture: AnyView<CatalogState> = any(GestureDetector(scaled)
        .hold_threshold_ms(CHARGE_HOLD_MS)
        .on_hold_progress(move |_s: &mut CatalogState, p: f64| {
            // Reset idiom for the Cancel staleness gap (see this fn's doc
            // comment, and `frust_widgets::gesture`'s module docs): always
            // trust the widget's latest observation rather than clamping it
            // to never decrease, so a fresh cycle's low-restarting first
            // observation overwrites any stale value left behind by an
            // earlier interrupted (Cancel'd) hold.
            progress_sig.set(p);
        })
        .on_long_press(move |_s: &mut CatalogState| {
            floated_sig.set(true);
            progress_sig.set(1.0);
        })
        .on_tap(move |_s: &mut CatalogState| {
            if floated_sig.get_untracked() {
                floated_sig.set(false);
                progress_sig.set(0.0);
            }
        }));

    block(vec![
        inflexible(label("05 Long-press → float")),
        inflexible(caption(if reduce {
            "reduced motion — press and hold; floats instantly at the threshold, no ring/scale"
        } else {
            "press and hold the row ~700ms — a charge ring fills; lift early to cancel"
        })),
        gap(6.0),
        inflexible(gesture),
    ])
}

// ---------------------------------------------------------------------------
// 06 — pull to refresh: rain burst
// ---------------------------------------------------------------------------

local_sig!(rain_active_sig, bool, false);
local_clock!(rain_elapsed_ms, rain_reset_clock);

/// The refresh zone's fixed size, in logical px (see the [module
/// docs](self)'s substitution note on why this is fixed rather than
/// measured-width-derived).
const RAIN_ZONE_W: f64 = 260.0;
const RAIN_ZONE_H: f64 = 56.0;
/// Column count across [`RAIN_ZONE_W`] (roughly `W/13`, rounded).
const RAIN_COLUMNS: u32 = 20;
/// Total burst length (reference: "~650ms").
const RAIN_BURST_MS: f64 = 650.0;
/// One column's fall duration.
const RAIN_FALL_MS: f64 = 400.0;
/// Max per-column start stagger — chosen so `stagger + RAIN_FALL_MS` never
/// exceeds [`RAIN_BURST_MS`] (`250 + 400 = 650`).
const RAIN_STAGGER_MOD_MS: u32 = 250;
/// Rain glyph charset — the reference's canvas draws raw ASCII "rain"; a small
/// binary/symbol-leaning set reads the same at this size.
const RAIN_CHARSET: &[u8] = b"01#$%mux";

/// A column's falling character — fixed per column index (not per frame), so
/// a column reads as one glyph raining down rather than flickering noise.
fn rain_char(col: u32) -> char {
    let r = lcg_next(col.wrapping_mul(0x9E37_79B1));
    RAIN_CHARSET[(r as usize) % RAIN_CHARSET.len()] as char
}

/// A column's deterministic start delay within the burst, in `0..RAIN_STAGGER_MOD_MS`.
fn rain_stagger(col: u32) -> f64 {
    (col.wrapping_mul(23) % RAIN_STAGGER_MOD_MS) as f64
}

/// 06 pull to refresh — rain burst: a 56px zone (a small [`ScrollView`] whose
/// [`ScrollView::on_refresh_release`] — the pull-to-refresh
/// trigger — fires the same handler as the "trigger refresh" button, since
/// the gesture needs a real touch/mouse drag past the top edge to verify on
/// desktop) bursts a fixed grid of falling glyph columns
/// ([`RAIN_COLUMNS`], each independently staggered/timed — the "honest
/// composition" choice over a shader quad, see the [module docs](self)),
/// fades out, then settles on "last updated just now".
///
/// [`ScrollView`]: frust::ScrollView
fn demo_rain_burst(state: &CatalogState) -> FlexChild<CatalogState> {
    // ORs the header animations-off toggle into the same local `reduce`
    // check every wall-clock demo already gates on — the "cheapest sound
    // wiring" `CatalogState::animations_enabled`'s doc comment describes,
    // not a second flag.
    let reduce = state.reduce_motion.get() || !state.animations_enabled.get();
    let active_sig = rain_active_sig();
    let active = active_sig.get();
    let elapsed = if active { rain_elapsed_ms() } else { 0.0 };
    // Reduced motion settles straight to the end-state — no falling glyphs.
    let running = active && !reduce && elapsed < RAIN_BURST_MS;

    let mut layers: Vec<AnyView<CatalogState>> =
        vec![any(SizedBox(Some(RAIN_ZONE_W), Some(RAIN_ZONE_H)).child(
            Image(solid_source(with_alpha(muted(), 0.05))).fit(ImageFit::Fill),
        ))];
    if running {
        for col in 0..RAIN_COLUMNS {
            let stagger = rain_stagger(col);
            let local = elapsed - stagger;
            if !(0.0..RAIN_FALL_MS).contains(&local) {
                continue;
            }
            let p = local / RAIN_FALL_MS;
            let x = -1.0 + 2.0 * (col as f64 + 0.5) / RAIN_COLUMNS as f64;
            let y = -1.0 + 2.0 * p;
            let op = (1.0 - p * 0.6).clamp(0.0, 1.0);
            layers.push(any(Align(
                Alignment::new(x, y),
                AnimatedOpacity(
                    op,
                    text(rain_char(col).to_string()).size(9.0).color(amber()),
                )
                .timing(ZERO),
            )));
        }
    }
    layers.push(any(Align(
        Alignment::CENTER,
        text(if running {
            "refreshing…"
        } else {
            "pull down to refresh"
        })
        .size(10.5)
        .color(if running { amber() } else { muted() }),
    )));

    let inner: AnyView<CatalogState> =
        any(SizedBox(Some(RAIN_ZONE_W), Some(RAIN_ZONE_H)).child(Stack(layers)));
    let zone: AnyView<CatalogState> = any(SizedBox(Some(RAIN_ZONE_W), Some(RAIN_ZONE_H)).child(
        scroll_view(inner).on_refresh_release(move |_s: &mut CatalogState| {
            rain_reset_clock();
            active_sig.set(true);
        }),
    ));

    let mut children = vec![
        inflexible(label("06 Pull to refresh — rain burst")),
        inflexible(caption(
            "pull down inside the zone (or tap trigger) — a burst of falling glyphs, then a timestamp",
        )),
        gap(6.0),
        inflexible(zone),
    ];
    if !running {
        children.push(inflexible(caption(if active {
            "last updated just now"
        } else {
            "last updated 2 minutes ago"
        })));
        children.push(gap(6.0));
    }
    children.push(inflexible(
        button("Trigger refresh", move |_s: &mut CatalogState| {
            rain_reset_clock();
            active_sig.set(true);
        })
        .small(),
    ));
    if running {
        children.push(frame_ticker());
    }
    block(children)
}

// ---------------------------------------------------------------------------
// 07 — live output indicator (waveform)
// ---------------------------------------------------------------------------

local_sig!(wave_live_sig, bool, false);
local_clock!(wave_elapsed_ms, wave_reset_clock_unused);

const WAVE_BAR_W: f64 = 3.0;
const WAVE_BAR_MAX_H: f64 = 12.0;
const WAVE_BAR_MIN_H: f64 = 4.0;
/// Per-bar animation-delay offset (reference: 0/.12/.24/.36s).
const WAVE_PHASE_STEP_MS: f64 = 120.0;
/// Reference `waveGo` keyframe period.
const WAVE_PERIOD_MS: f64 = 1000.0;

/// One bar's height at wall-clock `t_ms`: idle bars sit at the floor;
/// `reduce_motion` holds live bars at a static mid-height instead of
/// animating; otherwise a phase-offset sine oscillates `MIN..MAX`.
fn bar_height(t_ms: f64, idx: usize, live: bool, reduce: bool) -> f64 {
    if !live {
        return WAVE_BAR_MIN_H;
    }
    if reduce {
        return (WAVE_BAR_MIN_H + WAVE_BAR_MAX_H) / 2.0;
    }
    let phase_ms = t_ms - idx as f64 * WAVE_PHASE_STEP_MS;
    let x = (phase_ms / WAVE_PERIOD_MS) * std::f64::consts::TAU;
    let s = (x.sin() + 1.0) / 2.0;
    WAVE_BAR_MIN_H + (WAVE_BAR_MAX_H - WAVE_BAR_MIN_H) * s
}

/// One bar: a fixed-height slot, its filled portion bottom-anchored via top
/// padding (`CrossAxisAlignment` has no `End` variant — see the
/// [module docs](self)) so it visibly grows upward.
fn wave_bar(height: f64, color: Color) -> AnyView<CatalogState> {
    any(
        SizedBox(Some(WAVE_BAR_W), Some(WAVE_BAR_MAX_H)).child(Padding(
            EdgeInsets {
                top: WAVE_BAR_MAX_H - height,
                ..EdgeInsets::all(0.0)
            },
            SizedBox(Some(WAVE_BAR_W), Some(height))
                .child(Image(solid_source(color)).fit(ImageFit::Fill)),
        )),
    )
}

fn wave_row(name: &str, live: bool, reduce: bool, t: f64) -> AnyView<CatalogState> {
    let color = if live {
        amber()
    } else {
        with_alpha(amber(), 0.35)
    };
    let bars: Vec<FlexChild<CatalogState>> = (0..4)
        .map(|i| inflexible(wave_bar(bar_height(t, i, live, reduce), color)))
        .collect();
    any(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                flexible(1, text(name.to_string()).size(11.5)),
                inflexible(
                    FlexView::new(Axis::Horizontal, bars).cross_axis(CrossAxisAlignment::Center),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ))
}

/// 07 live output indicator: three panes, one toggleable — a small waveform
/// pulse on the pane currently "producing output" (paint-driven while
/// active, idle everywhere else).
fn demo_waveform(state: &CatalogState) -> FlexChild<CatalogState> {
    let live_sig = wave_live_sig();
    let live = live_sig.get();
    // ORs the header animations-off toggle into the same local `reduce`
    // check every wall-clock demo already gates on — the "cheapest sound
    // wiring" `CatalogState::animations_enabled`'s doc comment describes,
    // not a second flag.
    let reduce = state.reduce_motion.get() || !state.animations_enabled.get();
    let t = wave_elapsed_ms();

    let mut children = vec![
        inflexible(label("07 Live output indicator")),
        inflexible(caption(
            "a small waveform pulses on a pane that's actively producing output",
        )),
        gap(6.0),
        inflexible(wave_row("btop", false, reduce, t)),
        inflexible(wave_row("build-watch", live, reduce, t)),
        inflexible(wave_row("db-tunnel", false, reduce, t)),
        gap(6.0),
        inflexible(
            button(
                if live {
                    "Stop build-watch activity"
                } else {
                    "Toggle build-watch activity"
                },
                move |_s: &mut CatalogState| live_sig.set(!live_sig.get_untracked()),
            )
            .small(),
        ),
    ];
    if live && !reduce {
        children.push(frame_ticker());
    }
    block(children)
}

// ---------------------------------------------------------------------------
// 08 — copy burst
// ---------------------------------------------------------------------------

local_sig!(copy_active_sig, bool, false);
local_clock!(copy_elapsed_ms_raw, copy_reset_clock);

/// Field-flash fade length (reference: `.copy-field.flash` 400ms transitions).
const COPY_FLASH_MS: f64 = 400.0;
/// Drifting-particle animation length (reference: `.6s` particle transition).
const COPY_PARTICLE_MS: f64 = 650.0;
/// Inline mini-toast visible length (shortened from the reference's 1.8s to
/// keep the demo snappy).
const COPY_TOAST_MS: f64 = 1100.0;

/// 08 copy burst: clicking the field flashes it, sends a few characters
/// drifting up off the field, and shows a small inline "copied" toast — a
/// composition of [`AnimatedOpacity`]/[`Align`]/a local
/// [`badge`](frust_glyph::badge) rather than the global `toast_host` — this
/// demo's toast is deliberately scoped to the field, not the app-wide
/// overlay.
fn demo_copy_burst() -> FlexChild<CatalogState> {
    let active_sig = copy_active_sig();
    let active = active_sig.get();
    let elapsed = if active {
        copy_elapsed_ms_raw()
    } else {
        f64::MAX
    };
    let still_running = elapsed < COPY_TOAST_MS;

    let flash_opacity = if elapsed < COPY_FLASH_MS {
        1.0 - elapsed / COPY_FLASH_MS
    } else {
        0.0
    };
    let particle_p = (elapsed / COPY_PARTICLE_MS).clamp(0.0, 1.0);
    let particles_visible = elapsed < COPY_PARTICLE_MS;
    let toast_visible = elapsed < COPY_TOAST_MS;

    let flash_bg: AnyView<CatalogState> = any(AnimatedOpacity(
        flash_opacity,
        SizedBox(None, Some(44.0))
            .child(Image(solid_source(with_alpha(amber(), 0.18))).fit(ImageFit::Fill)),
    )
    .timing(ZERO));

    let field_content: AnyView<CatalogState> = any(Padding(
        EdgeInsets::all(11.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                flexible(1, text("100.71.31.57:50051").size(11.5)),
                inflexible(text("⧉").size(13.0).color(muted())),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ));

    let mut layers = vec![flash_bg, field_content];
    if particles_visible {
        const GLYPHS: [&str; 3] = ["0", "1", "#"];
        for (i, glyph) in GLYPHS.iter().enumerate() {
            let x = -0.5 + i as f64 * 0.5;
            let y = 1.0 - 2.2 * particle_p;
            let op = (1.0 - particle_p).max(0.0);
            layers.push(any(Align(
                Alignment { x, y },
                AnimatedOpacity(op, text(*glyph).size(10.0).color(amber())).timing(ZERO),
            )));
        }
    }
    let field = SizedBox(None, Some(44.0)).child(Stack(layers));

    let toast: AnyView<CatalogState> = any(AnimatedOpacity(
        if toast_visible { 1.0 } else { 0.0 },
        badge("copied to clipboard", BadgeVariant::Neutral),
    ));

    let mut children = vec![
        inflexible(label("08 Copy burst")),
        inflexible(caption(
            "click the field — it flashes and a couple characters drift up off it",
        )),
        gap(6.0),
        inflexible(GestureDetector(field).on_tap(move |_s: &mut CatalogState| {
            copy_reset_clock();
            active_sig.set(true);
        })),
        gap(6.0),
        inflexible(toast),
    ];
    if still_running {
        children.push(frame_ticker());
    }
    block(children)
}

/// Test-only seam (mirrors `pages::appbar`'s `pub fn open_*` precedent — "pub
/// on the `open_*` fns exists exactly for this seam", `tests/smoke.rs`'s own
/// doc comment on that helper): starts §01's heartbeat demo directly, the way
/// `pages::interactions::tests`' own in-file tests poke [`hb_active_sig`]
/// straight, letting a cross-crate integration test (`tests/smoke.rs`, which
/// only sees `pub` items) exercise the tap-to-play resume contract
/// without needing a real pointer-event dispatch through the whole page.
/// Not part of the page-fn contract (`pages/mod.rs`'s module docs) — never
/// called from `build`.
pub fn start_heartbeat_for_test() {
    hb_active_sig().set(true);
}

/// [`start_heartbeat_for_test`]'s twin for §07 live output — the smoke sweep
/// exercises at least two demos in both stopped/playing states, so a second
/// already-tap-to-play demo is needed alongside the heartbeat; §07's
/// `Toggle build-watch activity` button already flips [`wave_live_sig`] the
/// same way a real tap would.
pub fn start_waveform_for_test() {
    wave_live_sig().set(true);
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &CatalogState) -> AnyView<CatalogState> {
    any(FlexView::new(
        Axis::Vertical,
        vec![
            demo_heartbeat(state),
            gap(8.0),
            demo_attach(),
            gap(8.0),
            demo_boot(),
            gap(8.0),
            demo_token_scramble(state),
            gap(8.0),
            demo_charge_ring(state),
            gap(8.0),
            demo_rain_burst(state),
            gap(8.0),
            demo_waveform(state),
            gap(8.0),
            demo_copy_burst(),
        ],
    ))
}

#[cfg(test)]
mod tests {
    //! Headless test for the removed `pump()` widget — drives this page
    //! through the real `RenderRoot::rebuild`/`layout_with_text`/`paint` seam
    //! (mirrors `tests/smoke.rs`'s harness shape at module scope, kept local
    //! since these tests only cover this file) and asserts
    //! `PaintOutcome::needs_frame`.

    use std::any::Any;

    use frust_core::{FrameTime, RenderRoot};
    use frust_reactive::ReactiveRuntime;
    use frust_scene::GlyphRun;
    use frust_text::TextContext;
    use kurbo::{BezPath, Point, Rect, Size};
    use peniko::{Brush, Color};
    use reactive_graph::owner::Owner;

    use super::{AnyView, PaintScene, Set, page};
    use crate::CatalogState;

    /// A paint target that records nothing — only `RenderRoot::paint`'s
    /// returned `PaintOutcome::needs_frame` is under test here.
    struct NoopScene;

    impl PaintScene for NoopScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, _origin: Point, _size: Size, _radius: f64, _color: Color) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {}
        fn draw_image(&mut self, _data: &peniko::ImageData, _dest: Rect) {}
        fn fill_path(&mut self, _origin: Point, _path: &BezPath, _brush: &Brush) {}
        fn stroke_path(&mut self, _origin: Point, _path: &BezPath, _width: f64, _brush: &Brush) {}
    }

    /// Installs the reactive runtime and an ambient owner — mirrors
    /// `tests/smoke.rs::setup`, required before `CatalogState::new()` mints
    /// its `RwSignal`s.
    fn setup() -> Owner {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let ambient = Owner::new();
        ambient.set();
        ambient
    }

    /// One deterministic-`FrameTime` frame of this page, rebuilt/laid-out/
    /// painted through a real `RenderRoot` — returns whether the pass
    /// requested another frame.
    fn paint_needs_frame(
        root: &mut RenderRoot<CatalogState, AnyView<CatalogState>>,
        state: &mut CatalogState,
        tcx: &mut TextContext,
    ) -> bool {
        let mut logic = |s: &mut CatalogState| page(s);
        root.rebuild(&mut logic, state);
        let tcx_any: &mut dyn Any = tcx;
        root.layout_with_text(Size::new(390.0, 3000.0), tcx_any);
        let mut scene = NoopScene;
        root.paint(&mut scene, FrameTime::ZERO).needs_frame
    }

    /// At the demos' initial (nothing-tapped-yet) state, every
    /// `local_sig!` bool defaults `false` — including
    /// [`super::hb_active_sig`] now that §01 connection heartbeat is
    /// tap-to-play like every other demo on this page (previously the page's
    /// one exception that auto-repeated without input; see this file's
    /// module docs' Tap-to-play note). So a fresh page paint must report
    /// `needs_frame == false`: nothing on this page burns a frame until the
    /// user actually starts something.
    #[test]
    fn initial_page_paint_requests_zero_frames() {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();

        assert!(
            !paint_needs_frame(&mut root, &mut state, &mut tcx),
            "every demo starts stopped — an initial page paint must request no further frame"
        );
    }

    /// The core tap-to-play contract, exercised directly against
    /// [`super::hb_active_sig`] (the signal the row's tap handler flips):
    /// starting the heartbeat demo requests a frame, and stopping it again
    /// returns to zero — the load-bearing regression guard for the deleted
    /// `pump()`, now extended to the play/stop affordance: neither hack nor
    /// auto-repeat can leave `needs_frame` stuck `true` once the demo is
    /// stopped.
    #[test]
    fn heartbeat_tap_to_play_starts_and_stops_frame_requests() {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();

        assert!(!paint_needs_frame(&mut root, &mut state, &mut tcx));

        super::hb_active_sig().set(true);
        assert!(
            paint_needs_frame(&mut root, &mut state, &mut tcx),
            "starting the heartbeat demo must request the next frame"
        );

        super::hb_active_sig().set(false);
        assert!(
            !paint_needs_frame(&mut root, &mut state, &mut tcx),
            "stopping the heartbeat demo must return to zero frame requests"
        );
    }

    /// The `reduce_motion` counterpart to the test above: once the heartbeat
    /// is started, turning `reduce_motion` on must still unmount its ticker —
    /// proving the toggle force-stops it even while the underlying
    /// `hb_active_sig` stays `true`. Turning `reduce_motion` back off resumes
    /// the (never-reset) wall clock rather than restarting the cycle,
    /// restoring the ticker.
    #[test]
    fn reduce_motion_unmounts_and_restores_the_started_heartbeat_ticker() {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();

        super::hb_active_sig().set(true);
        assert!(
            paint_needs_frame(&mut root, &mut state, &mut tcx),
            "the started heartbeat demo must request its next frame"
        );

        state.reduce_motion.set(true);
        assert!(
            !paint_needs_frame(&mut root, &mut state, &mut tcx),
            "with reduce_motion on, the heartbeat ticker must unmount even though hb_active_sig \
             is still true — nothing should request a frame"
        );

        state.reduce_motion.set(false);
        assert!(
            paint_needs_frame(&mut root, &mut state, &mut tcx),
            "turning reduce_motion back off must restore the started heartbeat's frame request"
        );

        super::hb_active_sig().set(false);
    }

    /// The header animations-off toggle's counterpart to the test above:
    /// with `reduce_motion` left OFF but `animations_enabled` turned OFF, a
    /// started heartbeat's ticker must still unmount — proving the two
    /// flags OR together in the same `reduce` check
    /// (`crate::CatalogState::animations_enabled`'s doc comment) rather than
    /// the toggle needing `reduce_motion` set too.
    /// Flipping `animations_enabled` back on with `reduce_motion` still off
    /// restores the ticker, proving the toggle actually resumes animation
    /// rather than latching off.
    #[test]
    fn animations_toggle_unmounts_and_restores_the_started_heartbeat_ticker() {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();

        super::hb_active_sig().set(true);
        assert!(
            paint_needs_frame(&mut root, &mut state, &mut tcx),
            "the started heartbeat demo must request its next frame"
        );

        state.animations_enabled.set(false);
        assert!(
            !paint_needs_frame(&mut root, &mut state, &mut tcx),
            "with animations_enabled off (reduce_motion still off), the heartbeat ticker must \
             still unmount — nothing should request a frame"
        );

        state.animations_enabled.set(true);
        assert!(
            paint_needs_frame(&mut root, &mut state, &mut tcx),
            "turning animations back on (reduce_motion still off) must restore the started \
             heartbeat's ticker frame request"
        );

        super::hb_active_sig().set(false);
    }

    // -----------------------------------------------------------------------
    // Effect-slot headroom (heartbeat ring headroom clip, charge-ring glow width)
    // -----------------------------------------------------------------------

    use kurbo::{Affine, Shape};

    use super::{
        Axis, CHARGE_ROW_H, FlexView, HB_RING_DIAMETER, HB_RING_MS, HB_RING_SCALE_MAX,
        HB_RING_SLOT, demo_charge_ring, heartbeat_ring_state, heartbeat_ring_view,
    };

    /// A geometry-tracking `PaintScene` recorder: replays the SAME
    /// `push_transform`/`push_layer` transform-composition logic
    /// `frust_scene::SceneBuilder` uses internally (`docs/ARCHITECTURE.md`'s
    /// scene seam) so a stroked/filled path or a `push_layer` clip rect can be
    /// resolved into ABSOLUTE (post-scale) space right here, without a real
    /// `SceneBuilder`/GPU backend — the "recording fake" `docs/CODE_STANDARDS.md`'s
    /// Testing Patterns calls for. `origin`/`size`/`path` arguments a widget
    /// passes into `PaintScene` are already absolute-but-UNSCALED (paint-only
    /// `push_transform`/`push_layer` compositing never touches them — see
    /// `frust_widgets::motion::animated`'s module docs); this recorder applies
    /// whatever transform is active at each call to resolve the TRUE painted
    /// extent, exactly mirroring what `SceneBuilder::push_layer`/`stroke_path`
    /// record (`transform`, separate from the untransformed `rect`/`path`) for
    /// the real GPU backend to compose at encode time.
    #[derive(Default)]
    struct SlotRecorder {
        transform_stack: Vec<Affine>,
        /// Every `push_layer` call's clip rect, resolved to absolute space —
        /// an `AnimatedOpacity`'s own paint-time slot.
        layer_rects: Vec<Rect>,
        /// Every stroked/filled path's absolute bounding box, inflated by the
        /// stroke's half-width for `stroke_path` (a centerline-only bbox would
        /// under-count the real painted extent by `width / 2` per edge).
        draw_bboxes: Vec<Rect>,
        /// Every `draw_image` destination rect — already absolute per
        /// `PaintScene::draw_image`'s own contract (no further transform to
        /// apply, unlike `stroke_path`/`push_layer`'s local-then-transformed
        /// shape).
        image_rects: Vec<Rect>,
        /// Every `fill_rounded_rect` rect (e.g. `badge`'s background/dot) —
        /// used by the layout-stability test below to prove a sibling's own
        /// size stays small rather than stretching to a widened effect
        /// slot.
        rounded_rects: Vec<Rect>,
    }

    impl SlotRecorder {
        fn current(&self) -> Affine {
            *self.transform_stack.last().unwrap_or(&Affine::IDENTITY)
        }

        fn record_path(&mut self, origin: Point, path: &BezPath, inflate: f64) {
            let local = (Affine::translate((origin.x, origin.y)) * path.clone())
                .bounding_box()
                .inflate(inflate, inflate);
            self.draw_bboxes
                .push(self.current().transform_rect_bbox(local));
        }
    }

    impl PaintScene for SlotRecorder {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn draw_glyph_run(&mut self, _run: GlyphRun) {}
        fn draw_image(&mut self, _data: &peniko::ImageData, dest: Rect) {
            self.image_rects.push(dest);
        }
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, _radius: f64, _color: Color) {
            let local = Rect::new(
                origin.x,
                origin.y,
                origin.x + size.width,
                origin.y + size.height,
            );
            self.rounded_rects
                .push(self.current().transform_rect_bbox(local));
        }
        fn fill_path(&mut self, origin: Point, path: &BezPath, _brush: &Brush) {
            self.record_path(origin, path, 0.0);
        }
        fn stroke_path(&mut self, origin: Point, path: &BezPath, width: f64, _brush: &Brush) {
            self.record_path(origin, path, width / 2.0);
        }
        fn push_layer(&mut self, origin: Point, size: Size, _alpha: f32) {
            let local = Rect::new(
                origin.x,
                origin.y,
                origin.x + size.width,
                origin.y + size.height,
            );
            self.layer_rects
                .push(self.current().transform_rect_bbox(local));
        }
        fn pop_layer(&mut self) {}
        fn push_transform(&mut self, transform: Affine) {
            let composed = self.current() * transform;
            self.transform_stack.push(composed);
        }
        fn pop_transform(&mut self) {
            if self.transform_stack.len() > 1 {
                self.transform_stack.pop();
            }
        }
    }

    /// Lay out and paint a standalone `AnyView<CatalogState>` built fresh from
    /// `make_view` (not the whole page) at `size`, returning the recorded
    /// geometry — the isolation the tests below need to attribute every
    /// recorded draw/layer to the one effect under test, not the rest of the
    /// page. Mirrors `page`'s own `fn(&CatalogState) -> AnyView<CatalogState>`
    /// shape (`paint_needs_frame`'s `logic` above) rather than a pre-built
    /// `AnyView` — `AnyView` has no `Clone` impl, so a closure that builds a
    /// fresh one is the only shape a `RenderRoot::rebuild` callback can take.
    fn paint_view_into_recorder(
        mut make_view: impl FnMut(&CatalogState) -> AnyView<CatalogState>,
        size: Size,
    ) -> SlotRecorder {
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();
        let mut logic = |s: &mut CatalogState| make_view(s);
        root.rebuild(&mut logic, &mut state);
        let mut tcx = TextContext::new();
        let tcx_any: &mut dyn Any = &mut tcx;
        root.layout_with_text(size, tcx_any);
        let mut scene = SlotRecorder::default();
        root.paint(&mut scene, FrameTime::ZERO);
        scene
    }

    /// [`heartbeat_ring_state`]'s pure math at three fixed timestamps (T0
    /// start / T1 mid / T2 max extent) — no wall clock, no rendering.
    #[test]
    fn heartbeat_ring_state_start_mid_max_extent() {
        let (op0, scale0) = heartbeat_ring_state(0.0, false);
        assert!(
            (scale0 - 0.6).abs() < 1e-9,
            "T0 start: scale should be the 0.6x minimum"
        );
        assert!(
            (op0 - 0.7).abs() < 1e-9,
            "T0 start: opacity should be the 0.7 peak"
        );

        let (op1, scale1) = heartbeat_ring_state(HB_RING_MS / 2.0, false);
        assert!(
            (scale1 - 1.7).abs() < 1e-9,
            "T1 mid: scale should be the 0.6..2.8 midpoint"
        );
        assert!(
            (op1 - 0.35).abs() < 1e-9,
            "T1 mid: opacity should be the 0.7 midpoint"
        );

        let (op2, scale2) = heartbeat_ring_state(HB_RING_MS, false);
        assert!(
            (scale2 - HB_RING_SCALE_MAX).abs() < 1e-9,
            "T2 max extent: scale should settle at the 2.8x maximum"
        );
        assert_eq!(
            op2, 0.0,
            "T2 max extent: the ring has fully faded by the cycle boundary"
        );
    }

    /// The load-bearing regression guard: at T2 max extent
    /// (`ring_scale == HB_RING_SCALE_MAX`), the ring's stroked circle —
    /// resolved through the SAME `push_layer`(`AnimatedOpacity`) /
    /// `push_transform`(`AnimatedScale`) composition the real paint pass uses
    /// (see [`SlotRecorder`]'s doc comment) — must stay fully inside the
    /// `AnimatedOpacity`'s own recorded clip rect. With just an 18×18
    /// `AnimatedOpacity` slot and the scale applied *inside* it, this
    /// assertion fails: the clip rect stays fixed at 18×18 absolute while the
    /// circle's recorded transform grows with it, so the circle's bbox
    /// overflows the clip on every edge once `ring_scale > 1.0x`.
    #[test]
    fn heartbeat_ring_stays_within_its_headroom_slot_at_max_extent() {
        let _owner = setup();
        let (opacity, scale) = heartbeat_ring_state(HB_RING_MS, false);
        assert_eq!(scale, HB_RING_SCALE_MAX);

        let recorder = paint_view_into_recorder(
            |_s| heartbeat_ring_view(opacity.max(0.3), scale),
            Size::new(200.0, 200.0),
        );

        assert!(
            !recorder.layer_rects.is_empty(),
            "AnimatedOpacity must push a layer"
        );
        assert!(
            !recorder.draw_bboxes.is_empty(),
            "circular_progress must stroke the ring"
        );

        // The outermost (largest) layer rect is the AnimatedOpacity's own
        // paint-time slot — the headroom `HB_RING_SLOT` allocates.
        let slot = recorder
            .layer_rects
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .expect("at least one layer rect");
        assert!(
            (slot.width() - HB_RING_SLOT).abs() < 1.0 && (slot.height() - HB_RING_SLOT).abs() < 1.0,
            "the AnimatedOpacity slot should be ~{HB_RING_SLOT}x{HB_RING_SLOT}, got {}x{}",
            slot.width(),
            slot.height(),
        );

        for bbox in &recorder.draw_bboxes {
            assert!(
                slot.contains(bbox.origin()) && slot.contains(Point::new(bbox.x1, bbox.y1)),
                "ring geometry {bbox:?} must stay within its headroom slot {slot:?} at max \
                 extent (scale {scale}x, base diameter {HB_RING_DIAMETER}px)"
            );
        }
    }

    /// Layout stability: the ping ring's headroom fix grows
    /// ONLY the ring's own layout slot (`HB_RING_DIAMETER` → `HB_RING_SLOT`,
    /// `18px` → `52px`) — its `01 Connection heartbeat` row's
    /// `CrossAxisAlignment::Center` keeps every sibling (the "connected"
    /// badge, the latency text) at its own natural size, just re-centered
    /// within the now-taller row, not stretched to match. `demo_heartbeat`'s
    /// row layout is deterministic regardless of the wall clock (only the
    /// ring's *paint-time* opacity/scale react to `t`; every child's *layout*
    /// size is fixed), so this needs no controlled timestamp.
    #[test]
    fn heartbeat_ping_headroom_does_not_stretch_sibling_row_content() {
        let _owner = setup();
        let recorder = paint_view_into_recorder(
            |s| {
                super::any(FlexView::new(
                    Axis::Vertical,
                    vec![super::demo_heartbeat(s)],
                ))
            },
            Size::new(390.0, 400.0),
        );

        assert!(
            !recorder.rounded_rects.is_empty(),
            "the connected badge must paint its rounded background/dot"
        );
        for r in &recorder.rounded_rects {
            assert!(
                r.height() < HB_RING_SLOT,
                "sibling row content (badge background/dot, height {}) must keep its own small \
                 size rather than stretching to the ring's {HB_RING_SLOT}px headroom slot",
                r.height(),
            );
        }
    }

    /// The load-bearing regression guard for bug 3
    /// (catalog-animation-performance): renders `demo_charge_ring` with the
    /// row floated (glow visible) at a controlled viewport width and asserts
    /// the glow `Image`'s drawn destination rect actually spans the row's
    /// available width (`viewport - block()`'s 12px-per-side padding) at the
    /// fixed [`CHARGE_ROW_H`] height — not the pre-fix 1×1-natural-size
    /// collapse (a `SizedBox(None, ..)` width leaves `Image` unconstrained on
    /// that axis, so it falls back to its 1×1 solid-color source's natural
    /// size — see [`super::CHARGE_GLOW_ALPHA`]'s sibling fix in
    /// `demo_charge_ring`'s `glow_wash`).
    #[test]
    fn charge_ring_glow_fills_its_row_width() {
        let _owner = setup();
        const W: f64 = 390.0;
        const BLOCK_PADDING: f64 = 12.0 * 2.0;

        super::charge_floated_sig().set(true);
        let recorder = paint_view_into_recorder(
            |s| super::any(FlexView::new(Axis::Vertical, vec![demo_charge_ring(s)])),
            Size::new(W, 400.0),
        );
        super::charge_floated_sig().set(false);

        let expected_w = W - BLOCK_PADDING;
        let wide = recorder
            .image_rects
            .iter()
            .find(|r| r.width() > 10.0)
            .unwrap_or_else(|| {
                panic!(
                    "no wide glow image found among {:?} — the glow collapsed back to a hairline",
                    recorder.image_rects
                )
            });
        assert!(
            (wide.width() - expected_w).abs() < 1.0,
            "glow width should fill the row ({expected_w}px), got {}px",
            wide.width()
        );
        assert!(
            (wide.height() - CHARGE_ROW_H).abs() < 1.0,
            "glow height should stay {CHARGE_ROW_H}px, got {}px",
            wide.height()
        );
    }
}
