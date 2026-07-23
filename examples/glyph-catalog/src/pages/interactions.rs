//! Interactions section (glyph-refinements tasks 19 + 21): all eight moments
//! tied to actual muxr events — a connection heartbeat, a session-attach card
//! morph, a new-session boot sequence, a token-revoke character scramble, a
//! long-press charge ring, a pull-to-refresh rain burst, a live-output
//! waveform, and a copy-to-clipboard burst — reproducing
//! `../../glyph-design-system/research/glyph-interactions.html` §01-§08.
//! Task 19 landed §01/§02/§03/§07/§08 (composition-only, no new primitives);
//! task 21 (this addendum, §04/§05/§06) leans on task 15's
//! `GestureDetector::on_hold_progress`/`hold_threshold_ms` for the charge ring
//! and `ScrollView::on_refresh_release` for the rain burst.
//!
//! # No new framework primitives
//!
//! Every moment composes existing facade widgets only (research §7's survey).
//! Two deliberate substitutions from the task's suggested mapping, both
//! because the "obvious" primitive isn't reachable without a manifest change
//! this task's acceptance criteria rule out (`cargo build` catalog deps must
//! stay unchanged):
//!
//! - **§02 session attach** uses [`SharedAxis::Scaled`] instead of
//!   `TransitionPattern::ContainerTransform`: `ContainerTransform::new`/
//!   `from_source` takes a `kurbo::Rect`, and `kurbo` is not reachable from
//!   this crate's non-test code (only a `[dev-dependencies]` entry, used by
//!   `tests/smoke.rs`'s headless harness) — promoting it to a real
//!   `[dependencies]` edge would be exactly the "catalog deps unchanged"
//!   violation this task's Acceptance Criteria #2 forbids. `SharedAxis::Scaled`
//!   (incoming 0.80→1.0 scale + fade-through, no `Rect` needed) reads as the
//!   same "grows to fill" morph without it.
//! - **Per-frame demo state (§01/§03/§07/§08) reads a wall clock, not a
//!   timer signal.** `frust::spawn_local` + `tokio::time::sleep` is the
//!   framework's blessed interval idiom (`examples/huddle`'s
//!   `clean_signals::time::sleep` precedent), but that sleep helper lives in
//!   the (unavailable here) `clean-signals` crate, and this crate has no
//!   direct `tokio` dependency either. Instead, each self-driving demo reads
//!   `std::time::Instant::now()` directly inside its page fn (permitted: the
//!   "no wall clock" rule in `docs/CODE_STANDARDS.md`'s Theming & Animation
//!   Conventions binds `frust-core`/`frust-widgets`, not application code —
//!   `examples/huddle`/`examples/bubblebench` both already read `Instant`
//!   at this tier) and mounts a tiny [`FrameTicker`] — a hand-rolled
//!   `View`/`Widget` pair (this crate's `Cargo.toml` already carries
//!   `frust-core`/`kurbo` as real dependencies for `appbar.rs`'s
//!   `AnchorReporter`, the same documented escape hatch) whose only job is an
//!   unconditional `PaintCtx::request_frame` call in its own `paint` — to
//!   keep this page's `build` re-invoked every frame while an animation needs
//!   to keep advancing. **This replaces an earlier `pump()`**
//!   (catalog-animation-performance bug, task 03): an invisible
//!   `AnimatedOpacity(0.0)` wrapping an Indeterminate `circular_progress`,
//!   riding *that* unrelated widget's own always-request-frame paint
//!   behavior as a side effect instead of declaring the need directly.
//!   Mounting [`frame_ticker`] is the same per-frame-rebuild idiom the task
//!   cites (`frust_bench`'s `s6_text` `WidthPulse` pattern), adapted to a
//!   page fn with no `Component`/`PaintCtx` of its own — now via a widget
//!   that says what it's doing instead of exploiting one that doesn't.
//!
//! Task 21 adds three more, one further substitution and two implementation
//! choices left to the implementor's call (documented per the task):
//!
//! - **§04 scramble's noise source is a tiny LCG** ([`lcg_next`]), not
//!   `Math.random()` — deterministic per `(frame, char index)` so the same
//!   hold/frame always renders the same glitch text (reproducible in a test,
//!   unlike a true RNG), per the task's own "no `Math.random` analog needed"
//!   guidance.
//! - **§05's "shadow" is an amber wash layer, not `PaintScene::draw_shadow`.**
//!   That call is a `PaintCtx`/`Widget::paint` primitive with no facade
//!   equivalent reachable from application code (every demo *view* in this
//!   file composes existing facade widgets only — the sole exception is
//!   [`FrameTicker`] above, a narrow, documented escape hatch, not a general
//!   license to hand-roll widgets) — an `AnimatedOpacity`-faded [`Image`] wash
//!   behind the row (the same `solid_source` technique
//!   [`demo_copy_burst`]'s flash uses) reads as a comparable "lift" cue
//!   without it.
//! - **§06's rain is per-frame positioned glyph views on a fixed column
//!   grid** (the task's "honest-composition route"), not a `draw_shader` WGSL
//!   quad — consistent with every other demo on this page staying inside the
//!   plain widget-composition surface (no `frust_scene`/shader dependency to
//!   add to this crate's manifest, mirroring the §02 substitution's
//!   deps-unchanged constraint above). The column count is fixed rather than
//!   measured-width-derived (application code has no layout-pass access to
//!   its own resolved size), so the refresh zone itself is a fixed-width
//!   [`SizedBox`] ([`RAIN_ZONE_W`]) rather than filling the available width.
//!
//! # Reduced motion
//!
//! Every clock-driven demo checks `state.reduce_motion.get()` directly (the
//! `pattern_switcher`/`AnimatedOpacity`/`AnimatedScale` wrappers only
//! auto-collapse when resolving a *theme-default* timing, which these
//! wall-clock-driven demos deliberately bypass via an explicit
//! `Timing::Duration(Duration::ZERO, ..)` "immediate follower" — see [`ZERO`])
//! and skips the animated portion outright rather than fighting the built-in
//! collapse: the heartbeat ring/latency-roll, waveform bars, and copy-burst
//! particles all render their settled end-state instead, matching
//! `motion.rs`'s reduced-motion note ("every demo below collapses"). §04/§06
//! (also wall-clock-driven) follow the same rule — a revoke jumps straight to
//! the collapsed row, a refresh straight to "last updated just now", no
//! scramble/rain frames rendered. §05 is the one exception on this page: its
//! progress comes from `GestureDetectorView::on_hold_progress`, an
//! event-delivered (not wall-clock) observation with no theme-default timing
//! to bypass, so it needs no `ZERO`-timing workaround — reduced motion there
//! only drops the ring/scale visuals, not the underlying gesture contract.
//!
//! Structure mirrors `motion.rs`: `pub fn page(state)` + one `demo_*` fn per
//! moment inside `block(...)` scaffolds, thread-local demo state via the same
//! `local_sig!` macro.

use std::time::{Duration, Instant};

// Low-level escape hatch (see the module docs' [`FrameTicker`] note) —
// `frust-core`/`kurbo` back only `FrameTicker` below; every other widget in
// this file comes from the `frust` facade. Mirrors `appbar.rs`'s
// `AnchorReporter` — this crate's `Cargo.toml` already carries both as real
// dependencies for that use.
use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, View, Widget,
};
use kurbo::Size;

use frust::glyph::{BadgeVariant, TermLine, badge, glyph_card, term_block};
use frust::motion::patterns::SharedAxis;
use frust::motion::switcher::pattern_switcher;
use frust::motion::{AnimatedOpacity, AnimatedScale};
use frust::{
    Align, Alignment, AnyView, Axis, ButtonStyle, Color, CrossAxisAlignment, Curve, EdgeInsets,
    FlexChild, FlexView, GestureDetector, Get, GetUntracked, Image, ImageFit, ImageSource, Padding,
    ProgressValue, RwSignal, Set, SizedBox, Stack, Theme, Timing, Update, any, button,
    circular_progress, flexible, inflexible, keyed, scroll_view, text, use_context,
};

use crate::CatalogState;

/// Glyph accent amber — see `motion.rs`'s twin (each page file resolves its
/// own live-theme roles; not shared across files, matching
/// `navigation.rs`/`overlays.rs`'s existing per-file precedent).
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

/// The error/danger ink — [`demo_token_scramble`]'s glitching-token tint. See
/// [`amber`]'s twin.
fn error_ink() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .error
}

/// `color` with its alpha channel replaced — duplicated from
/// `frust-widgets::glyph::badge`'s crate-private helper of the same shape
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

local_clock!(hb_elapsed_ms, hb_reset_clock_unused);

/// Full cycle length: the ping ring fires, then latency rolls, then idles
/// until the next cycle — matching the reference's "runs automatically every
/// 3s" caption.
const HB_CYCLE_MS: f64 = 3000.0;
/// Ping-ring animation length (reference: 1.6s `pingRing` keyframe).
const HB_RING_MS: f64 = 1600.0;
/// Latency-roll animation length (reference: 10 steps × 30ms).
const HB_ROLL_MS: f64 = 300.0;
/// A small fixed cycle of "looks alive" latency samples — deterministic
/// rather than a `rand` dependency this crate doesn't have (Acceptance
/// Criteria #2: catalog deps unchanged).
const HB_LATENCIES: [f64; 6] = [36.0, 52.0, 41.0, 68.0, 29.0, 44.0];

/// 01 connection heartbeat: a radar-ping ring (a full-circle
/// [`circular_progress`] stroke under [`AnimatedOpacity`]/[`AnimatedScale`],
/// driven by a wall-clock read — no per-widget custom paint needed) plus a
/// rolling (not snapping) latency readout. The one demo on this page that
/// auto-repeats without input (Acceptance Criteria #3; the research footer's
/// sole exception).
fn demo_heartbeat(state: &CatalogState) -> FlexChild<CatalogState> {
    let reduce = state.reduce_motion.get();
    let elapsed = hb_elapsed_ms();
    let cycle = (elapsed / HB_CYCLE_MS).floor().max(0.0) as usize;
    let t = elapsed % HB_CYCLE_MS;

    let from_latency = HB_LATENCIES[cycle % HB_LATENCIES.len()];
    let to_latency = HB_LATENCIES[(cycle + 1) % HB_LATENCIES.len()];
    let rolling = !reduce && t < HB_ROLL_MS;
    let latency = if rolling {
        from_latency + (to_latency - from_latency) * (t / HB_ROLL_MS)
    } else {
        to_latency
    };

    let ring_active = !reduce && t < HB_RING_MS;
    let ring_p = if ring_active {
        (t / HB_RING_MS).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let ring_opacity = if ring_active {
        0.7 * (1.0 - ring_p)
    } else {
        0.0
    };
    let ring_scale = 0.6 + (2.8 - 0.6) * ring_p;

    let ring: AnyView<CatalogState> = any(AnimatedOpacity(
        ring_opacity,
        AnimatedScale(
            ring_scale,
            SizedBox(Some(18.0), Some(18.0))
                .child(circular_progress(ProgressValue::Determinate(1.0))),
        )
        .timing(ZERO),
    )
    .timing(ZERO));

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

    let mut children = vec![
        inflexible(label("01 Connection heartbeat")),
        inflexible(caption(if reduce {
            "reduced motion — ring suppressed; latency updates instantly"
        } else {
            "radar ping ring pulses every 3s; latency rolls rather than snapping"
        })),
        gap(6.0),
        inflexible(row),
    ];
    if !reduce {
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
// Shared PRNG (task 21 — §04/§06)
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
/// per-character loop. The frame's one allocation (Acceptance Criteria #1:
/// "no per-frame allocations beyond the rebuilt string").
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
    let reduce = state.reduce_motion.get();
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

/// 05 long-press → float: holding the row past [`CHARGE_HOLD_MS`] fills a
/// charge ring ([`circular_progress`]'s stroked-arc sweep, the same primitive
/// the heartbeat ping ring uses) via
/// `GestureDetectorView::on_hold_progress`/`GestureDetectorView::hold_threshold_ms`
/// (task 15); reaching the threshold "floats" the row (scale 1.03 + an amber
/// wash standing in for the reference's box-shadow — see the [module
/// docs](self)'s substitution note — + a chip); an early lift or slop-break
/// resets the ring to empty (the widget's own final-`0.0` contract). Tapping
/// a floated row drops it back down.
///
/// **Reset idiom (followup f2)**: `on_hold_progress`'s Cancel staleness gap
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
    let reduce = state.reduce_motion.get();
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
    let glow: AnyView<CatalogState> = any(AnimatedOpacity(
        if floated { 1.0 } else { 0.0 },
        SizedBox(None, Some(CHARGE_ROW_H))
            .child(Image(solid_source(with_alpha(amber(), 0.10))).fit(ImageFit::Fill)),
    )
    .timing(ZERO));

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
/// Column count across [`RAIN_ZONE_W`] (the task's "~W/13" guidance, rounded).
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
/// [`ScrollView::on_refresh_release`] — task 15/scroll's pull-to-refresh
/// trigger — fires the same handler as the "trigger refresh" button, since
/// the gesture needs a real touch/mouse drag past the top edge to verify on
/// desktop) bursts a fixed grid of falling glyph columns
/// ([`RAIN_COLUMNS`], each independently staggered/timed — the "honest
/// composition" choice over a shader quad, see the [module docs](self)),
/// fades out, then settles on "last updated just now".
///
/// [`ScrollView`]: frust::ScrollView
fn demo_rain_burst(state: &CatalogState) -> FlexChild<CatalogState> {
    let reduce = state.reduce_motion.get();
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
/// pulse on the pane currently "producing output" (paint-driven while active,
/// idle everywhere else — Acceptance Criteria #3).
fn demo_waveform(state: &CatalogState) -> FlexChild<CatalogState> {
    let live_sig = wave_live_sig();
    let live = live_sig.get();
    let reduce = state.reduce_motion.get();
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
/// [`badge`](frust::glyph::badge) rather than the global `toast_host`
/// (per the task's Details: "NOT the global host").
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
    //! T1 headless test for the `pump()` removal (catalog-animation-performance
    //! bug, task 03) — drives this page through the real
    //! `RenderRoot::rebuild`/`layout_with_text`/`paint` seam (mirrors
    //! `tests/smoke.rs`'s harness shape at module scope, since this task's
    //! scope is this file only) and asserts `PaintOutcome::needs_frame`.

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

    /// At the demos' initial (nothing-tapped-or-held-yet) state, every
    /// `local_sig!` bool defaults `false`, so §02-§08 are all idle and mount
    /// no [`super::frame_ticker`] — the sole exception is §01 connection
    /// heartbeat, the page's one demo documented to "auto-repeat without
    /// input" (this file's module docs' Reduced motion note), which mounts
    /// its ticker unconditionally while `!reduce_motion`. So a fresh page
    /// paint must report `needs_frame == true` (traceable to that one
    /// widget), and turning `reduce_motion` on — which gates the heartbeat's
    /// own ticker mount, per `demo_heartbeat`'s `if !reduce { ... }` — must
    /// flip it back to `false` with every other demo still idle. This is the
    /// load-bearing regression guard for the deleted `pump()`: that hack kept
    /// `needs_frame` `true` unconditionally regardless of `reduce_motion` or
    /// any demo's running state, since it rode an indeterminate spinner's
    /// paint behavior rather than a demo's own state.
    #[test]
    fn initial_state_requests_frames_only_from_the_heartbeat_ticker() {
        let _owner = setup();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<CatalogState, AnyView<CatalogState>> = RenderRoot::new();
        let mut state = CatalogState::new();

        assert!(
            paint_needs_frame(&mut root, &mut state, &mut tcx),
            "the heartbeat demo auto-repeats and must request the next frame at rest"
        );

        state.reduce_motion.set(true);
        assert!(
            !paint_needs_frame(&mut root, &mut state, &mut tcx),
            "with reduce_motion on, the heartbeat ticker is unmounted and every other demo is \
             still idle — nothing should request a frame"
        );
    }
}
