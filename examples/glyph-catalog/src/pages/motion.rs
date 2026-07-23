//! Motion section — the motion reference build's 11 live demos, plus the
//! duration/easing token table and a reduced-motion note tied to
//! `state.reduce_motion`.
//!
//! Every interactive demo needs state that survives the shell's per-frame
//! rebuild (a toggle's checked flag, the demo-card index, a replay counter),
//! but this page is a plain `page(&CatalogState)` function with no `Component`
//! state of its own and may not touch [`CatalogState`] (the `c01` scaffold
//! contract). So each demo's local state lives in a self-healing `thread_local!`
//! [`RwSignal`] cached across rebuilds — the same screen-local-state idiom
//! `examples/huddle`'s page functions use. `build` only ever *reads* those
//! signals (subscribing); every *write* happens in an event handler.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::glyph::TermLine;
use frust::glyph::{
    PaletteItem, accordion, command_palette, glyph_dialog, show_command_palette, show_glyph_dialog,
    tabs, term_block,
};
use frust::motion::patterns::{FadeScale, FadeThrough, GlyphSlide, SharedAxis, SlideDirection};
use frust::motion::switcher::pattern_switcher;
use frust::{
    AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked,
    MotionScheme, Padding, PageTransition, RwSignal, Set, SizedBox, Theme, TransitionSpec, Update,
    any, button, inflexible, keyed, switch, text,
};

use crate::CatalogState;

/// Glyph accent amber — the reference build's `--accent-amber`.
const AMBER: Color = Color::from_rgb8(0xFF, 0xB6, 0x27);
/// A muted caption ink for the per-demo timing notes.
const MUTED: Color = Color::from_rgb8(0x9A, 0x92, 0x80);

/// Defines a `fn $name() -> RwSignal<$ty>` returning a screen-local signal
/// cached in a `thread_local!` and lazily created (with self-heal, mirroring
/// `examples/huddle`'s `composer_for`): if a previously cached signal was
/// disposed with its owner (a headless test that re-enters), recreate it rather
/// than hand back a signal whose next `get`/`set` would panic.
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

local_sig!(toggle_sig, bool, false); // 02 toggle spring
local_sig!(tab_sig, usize, 0); // 03 tab indicator
local_sig!(accordion_sig, bool, false); // 04 accordion height
local_sig!(palette_query_sig, String, String::new()); // 07 palette live filter
local_sig!(log_replay_sig, usize, 0); // 09 staggered log reveal
local_sig!(card_sig, usize, 0); // 10 screen-transition card index
local_sig!(card_back_sig, bool, false); // 10 direction (Back vs Next)
local_sig!(pattern_sig, usize, 3); // 10 pattern picker (default GlyphSlide)
local_sig!(boot_replay_sig, usize, 0); // 11 boot sequence

/// A demo heading in accent amber.
fn label(s: impl Into<String>) -> AnyView<CatalogState> {
    any(text(s).size(13.0).color(AMBER))
}

/// A muted per-demo caption (timing notes, reduced-motion pointer).
fn caption(s: impl Into<String>) -> AnyView<CatalogState> {
    any(text(s).size(11.0).color(MUTED))
}

/// A fixed-height vertical spacer between demo blocks.
fn gap(h: f64) -> FlexChild<CatalogState> {
    inflexible(SizedBox(None, Some(h)))
}

/// Wrap a demo's rows in a padded vertical column (one showcase card).
fn block(children: Vec<FlexChild<CatalogState>>) -> FlexChild<CatalogState> {
    inflexible(Padding(
        EdgeInsets::all(12.0),
        FlexView::new(Axis::Vertical, children),
    ))
}

/// The duration + easing token table (5 durations, 3 easings) read from
/// `theme.motion` — the reference build's `--dur-*` / `--ease-*` vocabulary.
fn token_table(m: &MotionScheme) -> FlexChild<CatalogState> {
    let d = m.durations;
    let e = m.easing;
    let dur = |name: &str, ms: f64| inflexible(text(format!("{name:<11}{ms:>4.0}ms")).size(12.0));
    let ease = |name: &str, curve: String| {
        inflexible(text(format!("{name:<9}{curve}")).size(11.0).color(MUTED))
    };
    block(vec![
        inflexible(label("Motion tokens")),
        inflexible(caption("durations + easings, read from theme.motion")),
        gap(6.0),
        dur("instant", d.instant),
        dur("fast", d.fast),
        dur("base", d.base),
        dur("slow", d.slow),
        dur("deliberate", d.deliberate),
        gap(4.0),
        ease("spatial", format!("{:?}", e.spatial)),
        ease("effects", format!("{:?}", e.effects)),
        ease("exit", format!("{:?}", e.exit)),
    ])
}

/// The reduced-motion note: pointing at the shell header toggle, and reporting
/// the live state. With it ON, every demo below (the pattern switchers and the
/// navigator pushes) collapses to a fast linear crossfade — that collapse ships
/// in the framework (the pattern-switcher/navigator read `MotionScheme::reduce_motion`).
fn reduced_motion_note(state: &CatalogState) -> FlexChild<CatalogState> {
    let on = state.reduce_motion.get();
    let note = if on {
        "ON — every demo below collapses to a fast linear crossfade (no slide/scale)."
    } else {
        "OFF — flip “▶ Motion on” in the header to collapse all demos to a crossfade."
    };
    block(vec![
        inflexible(label("Reduced motion")),
        inflexible(caption(note)),
    ])
}

/// 01 press feedback: a Button (built-in 0.96 press scale) + caption.
fn demo_press() -> FlexChild<CatalogState> {
    block(vec![
        inflexible(label("01 Press feedback")),
        inflexible(caption("built-in 0.96 press scale — 100ms ease-exit")),
        gap(6.0),
        inflexible(button("Press me", |_s: &mut CatalogState| {})),
    ])
}

/// 02 toggle spring: a baseline switch (spatial knob translate + effects track).
fn demo_toggle() -> FlexChild<CatalogState> {
    let on = toggle_sig();
    let checked = on.get();
    block(vec![
        inflexible(label("02 Toggle spring")),
        inflexible(caption("220ms spatial knob + 150ms effects track")),
        gap(6.0),
        inflexible(switch(checked, move |_s: &mut CatalogState, v| on.set(v))),
    ])
}

/// 03 tab indicator: a small tabs instance (moving underline).
fn demo_tabs() -> FlexChild<CatalogState> {
    let sel = tab_sig();
    let s = sel.get();
    let labels = vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()];
    block(vec![
        inflexible(label("03 Tab indicator")),
        inflexible(caption(
            "underline slides — 220ms spatial (translate X + width)",
        )),
        gap(6.0),
        inflexible(tabs(labels, s, move |_s: &mut CatalogState, i| sel.set(i))),
    ])
}

/// 04 accordion height: one controlled accordion.
fn demo_accordion() -> FlexChild<CatalogState> {
    let open_sig = accordion_sig();
    let open = open_sig.get();
    block(vec![
        inflexible(label("04 Accordion height")),
        inflexible(caption("disclosure — 220ms spatial height")),
        gap(6.0),
        inflexible(
            accordion(
                "Details",
                text("Body content revealed on toggle.").size(12.0),
            )
            .open(open)
            .on_toggle(move |_s: &mut CatalogState| open_sig.set(!open_sig.get_untracked())),
        ),
    ])
}

/// 05 modal: a `show_glyph_dialog` push (enter 220ms scale .94→1 + fade / exit 150ms).
fn demo_modal() -> FlexChild<CatalogState> {
    block(vec![
        inflexible(label("05 Modal")),
        inflexible(caption("enter 220ms scale .94→1 + fade / exit 150ms")),
        gap(6.0),
        inflexible(button("Open modal", |s: &mut CatalogState| {
            show_glyph_dialog(
                &s.nav,
                || {
                    glyph_dialog()
                        .title("Confirm action")
                        .body("Enters at 220ms; exits at 150ms.")
                        .actions(vec![
                            any(button("Confirm", |s: &mut CatalogState| s.nav.pop())),
                            any(button("Cancel", |s: &mut CatalogState| s.nav.pop())
                                .style(ButtonStyle::Ghost)),
                        ])
                },
                |_s: &mut CatalogState, _r| {},
            );
        })),
    ])
}

/// 06 bottom sheet: a transparent `SlideUp` push (enter 340ms / exit 150ms).
fn demo_sheet() -> FlexChild<CatalogState> {
    block(vec![
        inflexible(label("06 Bottom sheet")),
        inflexible(caption(
            "enter 340ms translateY (heavy overshoot) / exit 150ms",
        )),
        gap(6.0),
        inflexible(button("Show bottom sheet", |s: &mut CatalogState| {
            s.nav.push_transparent_for_result(
                || {
                    any(Padding(
                        EdgeInsets::all(20.0),
                        FlexView::new(
                            Axis::Vertical,
                            vec![
                                inflexible(text("Bottom sheet").size(15.0).color(AMBER)),
                                gap(8.0),
                                inflexible(
                                    text("Slides up at 340ms; slides back at 150ms.").size(12.0),
                                ),
                                gap(12.0),
                                inflexible(button("Close", |s: &mut CatalogState| s.nav.pop())),
                            ],
                        ),
                    ))
                },
                TransitionSpec::duration(PageTransition::SlideUp),
                |_s: &mut CatalogState, _r| {},
            );
        })),
    ])
}

/// 07 command palette: a `show_command_palette` push with a live query filter
/// (enter 150ms scale .96→1 / exit 100ms — the snappy power-user path).
fn demo_palette() -> FlexChild<CatalogState> {
    block(vec![
        inflexible(label("07 Command palette")),
        inflexible(caption("enter 150ms scale .96→1 (snappy) / exit 100ms")),
        gap(6.0),
        inflexible(button("Open palette", |s: &mut CatalogState| {
            let query = palette_query_sig();
            show_command_palette(
                &s.nav,
                move || {
                    let q = query.get();
                    let needle = q.to_lowercase();
                    let all = [
                        "Toggle theme",
                        "Reduce motion",
                        "Go to Foundations",
                        "Replay log reveal",
                        "Open docs",
                    ];
                    let items: Vec<PaletteItem> = all
                        .iter()
                        .filter(|c| needle.is_empty() || c.to_lowercase().contains(needle.as_str()))
                        .map(|c| PaletteItem::new(*c))
                        .collect();
                    command_palette(
                        items,
                        move |_s: &mut CatalogState, t| query.set(t),
                        |s: &mut CatalogState, _i| s.nav.pop(),
                    )
                    .query(q)
                    .placeholder("Type to filter…")
                },
                |_s: &mut CatalogState, _i| {},
            );
        })),
    ])
}

/// 08 toast: trigger via the shell `toast_host` (2.4s auto-dismiss).
fn demo_toast() -> FlexChild<CatalogState> {
    block(vec![
        inflexible(label("08 Toast")),
        inflexible(caption("220ms in from top / 150ms out, 2.4s auto-dismiss")),
        gap(6.0),
        inflexible(button("Trigger toast", |s: &mut CatalogState| {
            s.toasts
                .update(|v| v.push("Saved — auto-dismisses in 2.4s".to_string()));
        })),
    ])
}

/// The log-reveal lines (09) — a fixed short build transcript.
fn log_lines() -> Vec<TermLine> {
    vec![
        TermLine::prompt("cargo run"),
        TermLine::output("compiling 42 modules"),
        TermLine::output("linking glyph-catalog"),
        TermLine::comment("done in 1.2s"),
    ]
}

/// 09 staggered log reveal: a staggered `term_block` + Replay. The `keyed`
/// wrapper remounts the block on each replay so its per-line stagger re-runs.
fn demo_log() -> FlexChild<CatalogState> {
    let replay = log_replay_sig();
    let n = replay.get();
    block(vec![
        inflexible(label("09 Staggered log reveal")),
        inflexible(caption("per-line 150ms effects, ~90ms stagger")),
        gap(6.0),
        inflexible(FlexView::new(
            Axis::Vertical,
            vec![keyed(n, term_block(log_lines()).staggered(true))],
        )),
        gap(6.0),
        inflexible(
            button("Replay", move |_s: &mut CatalogState| {
                replay.update(|c| *c += 1)
            })
            .small(),
        ),
    ])
}

/// A single numbered demo card for the screen-transition switcher (10).
fn card_view(n: usize) -> AnyView<CatalogState> {
    any(Padding(
        EdgeInsets::all(24.0),
        text(format!("Demo card {}", n + 1)).size(18.0).color(AMBER),
    ))
}

/// 10 screen transition: a mini `pattern_switcher` framed as Next/Back between
/// numbered cards (directional GlyphSlide), PLUS a pattern PICKER
/// (FadeThrough / SharedAxis::X / FadeScale / GlyphSlide) applied to the same
/// switcher — the composable-pattern showcase. The switcher plays its
/// transition on a card change; `reverse` mirrors it for a Back tap.
fn demo_screen() -> FlexChild<CatalogState> {
    let card = card_sig();
    let back = card_back_sig();
    let pat = pattern_sig();
    let c = card.get();
    let b = back.get();
    let p = pat.get();

    let picker = vec![
        "FadeThrough".to_string(),
        "SharedAxis::X".to_string(),
        "FadeScale".to_string(),
        "GlyphSlide".to_string(),
    ];

    // The same keyed switcher under each of the four patterns — type-erased so
    // the runtime pick composes onto one showcase.
    let switcher = match p {
        0 => any(pattern_switcher(c, FadeThrough, card_view(c)).reverse(b)),
        1 => any(pattern_switcher(c, SharedAxis::X, card_view(c)).reverse(b)),
        2 => any(pattern_switcher(c, FadeScale, card_view(c)).reverse(b)),
        _ => {
            any(pattern_switcher(c, GlyphSlide::new(SlideDirection::Left), card_view(c)).reverse(b))
        }
    };

    block(vec![
        inflexible(label("10 Screen transition")),
        inflexible(caption(
            "Next/Back — 340ms slide-in 16px + fade / 150ms exit, directional",
        )),
        gap(6.0),
        inflexible(tabs(picker, p, move |_s: &mut CatalogState, i| pat.set(i))),
        gap(6.0),
        inflexible(switcher),
        gap(6.0),
        inflexible(FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(
                    button("Back", move |_s: &mut CatalogState| {
                        back.set(true);
                        card.update(|v| *v = (*v + 2) % 3);
                    })
                    .style(ButtonStyle::Secondary),
                ),
                inflexible(SizedBox(Some(8.0), None)),
                inflexible(button("Next", move |_s: &mut CatalogState| {
                    back.set(false);
                    card.update(|v| *v = (*v + 1) % 3);
                })),
            ],
        )),
    ])
}

/// The boot-sequence lines (11) — a cold-start transcript.
fn boot_lines() -> Vec<TermLine> {
    vec![
        TermLine::comment("cold start"),
        TermLine::output("init kernel"),
        TermLine::output("mount filesystems"),
        TermLine::output("start services"),
        TermLine::prompt("ready"),
    ]
}

/// 11 boot sequence: `term_block`'s stagger timing is fixed (90ms/line — no
/// custom `GlyphStagger { per_item_delay }` is exposed on `term_block`, whose
/// only knob is `.staggered(bool)`), so the reference build's slower ~260ms/line
/// boot cadence is **N/A** here; this shows the standard staggered cascade
/// labeled as the boot sequence, with a Replay to re-run it.
fn demo_boot() -> FlexChild<CatalogState> {
    let boot = boot_replay_sig();
    let n = boot.get();
    block(vec![
        inflexible(label("11 Boot sequence")),
        inflexible(caption(
            "term_block stagger is fixed at 90ms/line — custom 260ms/line not exposed; standard cascade shown",
        )),
        gap(6.0),
        inflexible(FlexView::new(
            Axis::Vertical,
            vec![keyed(n, term_block(boot_lines()).staggered(true))],
        )),
        gap(6.0),
        inflexible(
            button("Replay", move |_s: &mut CatalogState| {
                boot.update(|c| *c += 1)
            })
            .small(),
        ),
    ])
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &CatalogState) -> AnyView<CatalogState> {
    // Durations/easings come from the canonical Glyph motion scheme (the app
    // forces `Theme::glyph_baseline()`; only its `reduce_motion` flag toggles,
    // never these token values).
    let motion = Theme::glyph_baseline().motion;

    any(FlexView::new(
        Axis::Vertical,
        vec![
            token_table(&motion),
            gap(8.0),
            reduced_motion_note(state),
            gap(8.0),
            demo_press(),
            gap(8.0),
            demo_toggle(),
            gap(8.0),
            demo_tabs(),
            gap(8.0),
            demo_accordion(),
            gap(8.0),
            demo_modal(),
            gap(8.0),
            demo_sheet(),
            gap(8.0),
            demo_palette(),
            gap(8.0),
            demo_toast(),
            gap(8.0),
            demo_log(),
            gap(8.0),
            demo_screen(),
            gap(8.0),
            demo_boot(),
        ],
    ))
}
