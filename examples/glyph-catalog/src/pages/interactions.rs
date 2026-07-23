//! Interactions section (glyph-refinements task 19): five moments tied to
//! actual muxr events — a connection heartbeat, a session-attach card morph,
//! a new-session boot sequence, a live-output waveform, and a copy-to-clipboard
//! burst — reproducing
//! `../../glyph-design-system/research/glyph-interactions.html` §01/§02/§03/
//! §07/§08 (the remaining three — §04 scramble, §05 charge ring, §06 rain
//! burst — are task 21, appended to this same file).
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
//!   at this tier) and mounts a tiny, invisible [`pump`] — a
//!   [`frust::motion::AnimatedOpacity`] wrapping a
//!   [`circular_progress`]`(`[`ProgressValue::Indeterminate`]`)`, whose
//!   `Indeterminate` arm unconditionally calls `PaintCtx::request_frame` every
//!   paint — to keep this page's `build` re-invoked every frame while an
//!   animation needs to keep advancing. This is the same per-frame-rebuild
//!   idiom the task cites (`frust_bench`'s `s6_text` `WidthPulse` pattern),
//!   adapted to a page fn with no `Component`/`PaintCtx` of its own.
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
//! `motion.rs`'s reduced-motion note ("every demo below collapses").
//!
//! Structure mirrors `motion.rs`: `pub fn page(state)` + one `demo_*` fn per
//! moment inside `block(...)` scaffolds, thread-local demo state via the same
//! `local_sig!` macro.

use std::time::{Duration, Instant};

use frust::glyph::{BadgeVariant, TermLine, badge, glyph_card, term_block};
use frust::motion::patterns::SharedAxis;
use frust::motion::switcher::pattern_switcher;
use frust::motion::{AnimatedOpacity, AnimatedScale};
use frust::{
    Align, Alignment, AnyView, Axis, ButtonStyle, Color, CrossAxisAlignment, Curve, EdgeInsets,
    FlexChild, FlexView, GestureDetector, Get, GetUntracked, Image, ImageFit, ImageSource, Padding,
    ProgressValue, RwSignal, Set, SizedBox, Stack, Theme, Timing, Update, any, button,
    circular_progress, flexible, inflexible, keyed, text, use_context,
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

/// An invisible, always-requesting-a-frame pump: an
/// [`AnimatedOpacity`]`(0.0, ..)` wrapping a
/// [`circular_progress`]`(`[`ProgressValue::Indeterminate`]`)`, whose
/// `Indeterminate` arm calls `PaintCtx::request_frame` unconditionally every
/// paint (`material::progress`'s indeterminate spinner loop) — see the
/// [module docs](self)'s wall-clock note for why a clock-driven demo needs
/// this to keep its `page()` call re-invoked.
fn pump() -> FlexChild<CatalogState> {
    inflexible(AnimatedOpacity(
        0.0,
        SizedBox(Some(0.0), Some(0.0)).child(circular_progress(ProgressValue::Indeterminate)),
    ))
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
        children.push(pump());
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
            card_children.push(pump());
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
        children.push(pump());
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
        children.push(pump());
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
            demo_waveform(state),
            gap(8.0),
            demo_copy_burst(),
        ],
    ))
}
