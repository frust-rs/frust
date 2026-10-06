//! Motion · Shader — `frust_beui::components::shader_background`, live.
//!
//! The one gallery page whose content is drawn by the GPU rather than by the
//! scene's own primitives: every tile here is a `Command::ShaderQuad` carrying a
//! hand-written WGSL fragment program, rendered by the engine into an offscreen
//! target and composited over the page.
//!
//! # What the page shows
//!
//! - a **featured** tile, one tab per ported variant, with live controls:
//!   animate on/off (the static mode), a three-rung speed picker, and a toggle
//!   that drops the tile onto a tinted card so a translucent variant's
//!   compositing is visible rather than asserted;
//! - a **contact sheet** of all five variants at once, deliberately frozen
//!   (`.animate(false)`) so the page holds one animating shader, not six;
//! - the **deferred** list: the sixteen upstream slugs this port does not cover.
//!
//! # Page-local state
//!
//! `pages::motion::shader::page()` takes no state — `main.rs`'s `page_body`
//! calls it with nothing, and this page may not add a field to
//! [`crate::AppState`]. Its four controls therefore live in self-healing
//! `thread_local!` [`RwSignal`]s cached across rebuilds, the same screen-local
//! state idiom `examples/glyph-catalog`'s own page functions use: `page()` only
//! ever *reads* them (subscribing the build), and every *write* happens in a
//! control's event handler.

use frust::{
    AnyView, Color, CrossAxisAlignment, Get, GetUntracked, Row, RwSignal, Set, SizedBox, Theme,
    any, container, text, use_context,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::shader_background::{
    DEFERRED_VARIANTS, SHADER_EFFECTS_KILL_SWITCH, ShaderBackgroundVariant, shader_background,
};
use frust_beui::components::switch::switch;
use frust_beui::components::tabs::{TabsVariant, tabs, tabs_tab};
use frust_beui::style::with_alpha;

use crate::AppState;
use crate::nav::{caption, heading};

/// The featured tile's footprint.
const FEATURE_SIZE: (f64, f64) = (560.0, 260.0);

/// One contact-sheet thumbnail's footprint.
const THUMB_SIZE: (f64, f64) = (150.0, 96.0);

/// The tile corner radius — beUI's `rounded-2xl`, which is what upstream's own
/// preview frames its shader in.
const TILE_RADIUS: f64 = 16.0;

/// The three speed rungs the picker offers, upstream's preset (`0.4`) in the
/// middle.
const SPEEDS: [(&str, f32); 3] = [("0.15x", 0.15), ("0.4x", 0.4), ("1.2x", 1.2)];

/// Defines `fn $name() -> RwSignal<$ty>`, a page-local signal cached in a
/// `thread_local!` and lazily (re)created — the self-healing shape
/// `examples/glyph-catalog`'s `local_sig!` uses, so a signal disposed with its
/// owner (a headless test that re-enters) is rebuilt rather than handed back
/// dead.
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

local_sig!(variant_sig, usize, 0); // which variant the featured tile shows
local_sig!(animate_sig, bool, true); // the static-mode switch
local_sig!(speed_sig, usize, 1); // an index into `SPEEDS`
local_sig!(over_card_sig, bool, false); // drop the tile onto a tinted card

/// The live theme, or the catalog's own baseline before a context exists.
fn theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_beui::theme)
}

/// The tint the "over a tinted card" toggle paints behind a tile — beUI's
/// accent (`ColorScheme::tertiary`) washed back far enough to read as a card
/// rather than as a fill.
fn card_tint() -> Color {
    with_alpha(theme().scheme().tertiary, 0.35)
}

/// A muted line of small print.
fn muted(body: impl Into<String>) -> AnyView<AppState> {
    any(caption(body.into()).color(theme().scheme().on_surface_variant))
}

/// A fixed spacer.
fn gap(height: f64) -> AnyView<AppState> {
    any(SizedBox(None, Some(height)))
}

/// One shader tile: the background at `variant`, framed at `size`, optionally
/// over a tinted card.
fn tile(
    variant: ShaderBackgroundVariant,
    animate: bool,
    speed: f32,
    size: (f64, f64),
    over_card: bool,
) -> AnyView<AppState> {
    let background = shader_background::<AppState>(variant)
        .animate(animate)
        .speed(speed);
    let mut frame = container(background)
        .size_centered(size.0, size.1)
        .radius(TILE_RADIUS);
    if over_card {
        // The fill sits *behind* the shader quad, so a variant with a
        // translucent backdrop (`dot-grid`) composites over it — which is the
        // whole point of the engine's premultiplied-alpha contract.
        frame = frame.fill(card_tint());
    }
    any(frame)
}

/// The featured section: a tab per variant, each showing the live tile.
fn featured() -> AnyView<AppState> {
    let variant = variant_sig();
    let animate = animate_sig();
    let speed = speed_sig();
    let over_card = over_card_sig();

    let active = variant.get().min(ShaderBackgroundVariant::ALL.len() - 1);
    let animating = animate.get();
    let rung = speed.get().min(SPEEDS.len() - 1);
    let on_card = over_card.get();

    let panels: Vec<_> = ShaderBackgroundVariant::ALL
        .into_iter()
        .map(|v| {
            tabs_tab(
                v.slug(),
                v.label(),
                tile(
                    v,
                    animating && v.animates(),
                    SPEEDS[rung].1,
                    FEATURE_SIZE,
                    on_card,
                ),
            )
        })
        .collect();

    any(tabs(
        ShaderBackgroundVariant::ALL[active].slug(),
        panels,
        move |_: &mut AppState, value: String| {
            if let Some(index) = ShaderBackgroundVariant::ALL
                .iter()
                .position(|v| v.slug() == value)
            {
                variant.set(index);
            }
        },
    )
    .variant(TabsVariant::Pill))
}

/// The control row under the featured tile.
fn controls() -> AnyView<AppState> {
    let animate = animate_sig();
    let speed = speed_sig();
    let over_card = over_card_sig();
    let active = variant_sig()
        .get()
        .min(ShaderBackgroundVariant::ALL.len() - 1);
    let variant = ShaderBackgroundVariant::ALL[active];
    let rung = speed.get().min(SPEEDS.len() - 1);

    let mut row: Vec<AnyView<AppState>> = vec![
        any(text("Animate").size(13.0)),
        any(SizedBox(Some(8.0), None)),
        any(switch(
            animate.get() && variant.animates(),
            move |_: &mut AppState, on| animate.set(on),
        )
        .label("Animate the featured shader")
        // A variant with no clock cannot be animated — the switch says so
        // rather than lying about what it does.
        .disabled(!variant.animates())),
        any(SizedBox(Some(24.0), None)),
        any(text("Speed").size(13.0)),
        any(SizedBox(Some(8.0), None)),
    ];
    for (index, (label, _)) in SPEEDS.iter().enumerate() {
        let tone = if index == rung {
            ButtonTone::Primary
        } else {
            ButtonTone::Outline
        };
        row.push(any(button(*label, move |_: &mut AppState| {
            speed.set(index)
        })
        .tone(tone)
        .size(ButtonSize::Sm)));
        row.push(any(SizedBox(Some(6.0), None)));
    }
    row.push(any(SizedBox(Some(18.0), None)));
    row.push(any(text("Over a tinted card").size(13.0)));
    row.push(any(SizedBox(Some(8.0), None)));
    row.push(any(switch(over_card.get(), move |_: &mut AppState, on| {
        over_card.set(on)
    })
    .label("Show the tile over a tinted card")));

    any(Row(row).cross_axis(CrossAxisAlignment::Center))
}

/// The contact sheet: every ported variant at once, frozen.
fn contact_sheet() -> AnyView<AppState> {
    let mut row: Vec<AnyView<AppState>> = Vec::new();
    for (index, variant) in ShaderBackgroundVariant::ALL.into_iter().enumerate() {
        if index > 0 {
            row.push(any(SizedBox(Some(12.0), None)));
        }
        row.push(any(frust::column()
            .child(tile(variant, false, 0.0, THUMB_SIZE, false))
            .child(gap(6.0))
            .child(text(variant.label().to_string()).size(12.0))
            .child(muted(variant.slug().to_string()))));
    }
    any(Row(row).cross_axis(CrossAxisAlignment::Start))
}

/// The Shader page.
pub fn page() -> AnyView<AppState> {
    any(frust::column()
        .child(heading("Motion \u{b7} Shader"))
        .child(gap(8.0))
        .child(muted(
            "shader_background \u{2014} beUI's `shader-background`, ported as hand-written WGSL. \
             Each tile is one Command::ShaderQuad: the engine compiles the fragment program once, \
             renders it into an offscreen target and composites it premultiplied, so a variant \
             with a translucent backdrop shows what is behind it.",
        ))
        .child(gap(20.0))
        .child(featured())
        .child(gap(14.0))
        .child(controls())
        .child(gap(10.0))
        .child(muted(
            "A static variant never references the shader clock and never asks for a frame; an \
             animating one asks for a paced cosmetic-loop frame, and reduced motion freezes it at \
             t = 0 (upstream's own speed: 0 rule).",
        ))
        .child(gap(28.0))
        .child(text("All five variants").size(16.0))
        .child(gap(4.0))
        .child(muted(
            "Frozen thumbnails \u{2014} the page animates one shader at a time, not six.",
        ))
        .child(gap(12.0))
        .child(contact_sheet())
        .child(gap(28.0))
        .child(text("Deferred variants").size(16.0))
        .child(gap(4.0))
        .child(muted(format!(
            "Upstream ships 21 variants; five are ported. The remaining sixteen are each a \
             distinct shader needing its own WGSL and its own GPU verification, and are not \
             ported: {}.",
            DEFERRED_VARIANTS.join(", ")
        )))
        .child(gap(10.0))
        .child(muted(format!(
            "Escape hatch: {SHADER_EFFECTS_KILL_SWITCH}=1 turns the engine's shader-effect path \
             off entirely; every tile above then degrades to a flat fill of its own backdrop \
             token (and to nothing at all where that backdrop is transparent).",
        ))))
}
