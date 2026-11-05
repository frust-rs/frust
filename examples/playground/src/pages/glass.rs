//! Glass section: N `glass_surface` tiles over continuously moving,
//! high-contrast content, with the per-frame backdrop cap selectable at
//! runtime. It is the on-device vehicle for judging backdrop-blur frame cost
//! at caps 4/6/8 and for eyeballing the glass painting itself; it is a
//! showcase page, not a benchmark harness.
//!
//! # Controls
//!
//! - **Surfaces** (`-` / `+` buttons, 1..=8): how many tiles are laid out in
//!   a four-wide grid. Each tile is one `glass_surface`, so each records one
//!   backdrop blur. (A segmented control tops out at five segments, hence
//!   the buttons.)
//! - **Cap** (segmented, 4 / 6 / 8): calls [`frust::set_max_backdrops_per_frame`]
//!   and the page shows the value the engine reports back via
//!   [`frust::max_backdrops_per_frame`].
//! - **Tier** (segmented, Control / Bar / Chrome): picks which explicit
//!   [`GlassMaterial`] the tiles use (see [`glass_material`]).
//!
//! # Refusal is expected past the cap
//!
//! The engine refuses a frame that records more backdrops than the cap
//! rather than drawing it partly blurred. With more surfaces than the cap the
//! page therefore shows a blank or last-good frame; the status line says so.
//! Raise the cap or drop a surface to bring the frame back.
//!
//! # Animated backdrop
//!
//! [`StripesView`] paints vertical stripes whose phase derives from
//! [`PaintCtx::frame_time`] (the scaled animation clock) and requests a
//! regular frame on every paint ([`PaintCtx::request_frame`]): the point is to
//! measure sustained per-frame cost, so the loop must not be paced down. With
//! the shell's animations toggle off it paints a still phase and stops
//! requesting frames.
//!
//! # Explicit material
//!
//! The playground seeds an opaque-glass theme, so the tiles carry an explicit
//! material override; otherwise they would paint the opaque fallback and no
//! blur would be recorded.

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::kurbo::Point;
use frust::{
    ButtonStyle, Color, EdgeInsets, Get, GlassFill, GlassMaterial, GlassTier, Padding, Set,
    ShadowSpec, SizedBox, Theme, button, column, glass_surface, max_backdrops_per_frame, row,
    set_max_backdrops_per_frame, stack, text, use_context,
};
use frust_material::{segment, segmented_button};

use crate::PlaygroundState;

/// The fewest surfaces the stepper allows.
pub const MIN_SURFACES: usize = 1;
/// The most surfaces the stepper allows.
pub const MAX_SURFACES: usize = 8;
/// The caps the segmented control offers.
pub const CAP_CHOICES: [usize; 3] = [4, 6, 8];

/// Tiles per grid row.
const TILES_PER_ROW: usize = 4;
/// Square tile edge, logical px.
const TILE_SIZE: f64 = 76.0;
/// Gap between tiles, logical px.
const TILE_GAP: f64 = 8.0;
/// Tile corner radius, logical px.
const TILE_RADIUS: f64 = 16.0;
/// Height of the stripes area, logical px (two tile rows plus padding).
const STAGE_HEIGHT: f64 = 2.0 * TILE_SIZE + TILE_GAP + 48.0;
/// Width of one stripe, logical px; the pattern repeats every two stripes.
const STRIPE_WIDTH: f64 = 24.0;
/// Horizontal stripe speed, logical px per second.
const STRIPE_SPEED: f64 = 60.0;

/// The dark stripe color.
const STRIPE_DARK: Color = Color::from_rgb8(0x10, 0x10, 0x1C);
/// The bright stripe color.
const STRIPE_BRIGHT: Color = Color::from_rgb8(0xF2, 0xC2, 0x30);

/// The explicit material a tile paints for `tier`: a real blur intent (a
/// Gaussian sigma in logical px), one translucent wash for both brightnesses,
/// a 0.3 hairline and a drop shadow. Heavier tiers blur more and wash more.
pub fn glass_material(tier: GlassTier) -> GlassMaterial {
    let (blur, wash_alpha) = match tier {
        GlassTier::Control => (12.0, 0.10),
        GlassTier::Bar => (12.0, 0.25),
        GlassTier::Chrome => (24.0, 0.45),
    };
    let wash = vec![GlassFill::new(1.0, 1.0, 1.0, wash_alpha)];
    GlassMaterial {
        blur_radius_intent: blur,
        fills_light: wash.clone(),
        fills_dark: wash,
        hairline_alpha: 0.3,
        shadow: ShadowSpec {
            y_offset: 4.0,
            blur_std_dev: 8.0,
            color_alpha: 0.25,
        },
    }
}

/// Whether a frame with `surfaces` backdrops is refused under `cap`.
pub fn is_refused(surfaces: usize, cap: usize) -> bool {
    surfaces > cap
}

/// The status line for `surfaces` tiles under an applied `cap`.
pub fn cap_status(surfaces: usize, cap: usize) -> String {
    if is_refused(surfaces, cap) {
        format!(
            "{surfaces} surfaces > cap {cap}: the frame is refused whole \
             (expected: blank or last frame)"
        )
    } else {
        format!("{surfaces} surfaces <= cap {cap}: the frame is drawn")
    }
}

/// The stripes' horizontal phase at animation time `secs`, in `[0, 2 * STRIPE_WIDTH)`.
pub fn stripe_phase(secs: f64) -> f64 {
    (secs * STRIPE_SPEED).rem_euclid(2.0 * STRIPE_WIDTH)
}

/// A tier's display name.
fn tier_name(tier: GlassTier) -> &'static str {
    match tier {
        GlassTier::Control => "Control",
        GlassTier::Bar => "Bar",
        GlassTier::Chrome => "Chrome",
    }
}

/// The moving high-contrast backdrop. See the [module docs](self).
struct StripesView {
    animate: bool,
}

impl<State: 'static> View<State> for StripesView {
    type Element = StripesWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> StripesWidget {
        StripesWidget {
            animate: self.animate,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut StripesWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.animate = self.animate;
        ChangeFlags::PAINT
    }
}

struct StripesWidget {
    animate: bool,
}

impl Widget for StripesWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::new(f64::INFINITY, STAGE_HEIGHT))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let origin = ctx.origin();
        let size = ctx.size();
        scene.fill_rect(origin, size, STRIPE_DARK);
        let phase = if self.animate {
            stripe_phase(ctx.frame_time().as_secs_f64())
        } else {
            0.0
        };
        let period = 2.0 * STRIPE_WIDTH;
        let mut x = phase - period;
        while x < size.width {
            let left = x.max(0.0);
            let right = (x + STRIPE_WIDTH).min(size.width);
            if right > left {
                scene.fill_rect(
                    Point::new(origin.x + left, origin.y),
                    Size::new(right - left, size.height),
                    STRIPE_BRIGHT,
                );
            }
            x += period;
        }
        if self.animate {
            ctx.request_frame();
        }
    }
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let count = state.glass_count.get().clamp(MIN_SURFACES, MAX_SURFACES);
    let selected_cap = state.glass_cap.get();
    let tier = state.glass_tier.get();
    let animate = state.animations_enabled.get();
    let applied_cap = max_backdrops_per_frame();

    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let accent = scheme.primary;
    let muted = scheme.on_surface_variant;
    let ink = scheme.on_surface;

    let material = glass_material(tier);
    let tiles = (0..count).collect::<Vec<_>>();
    let mut grid = column();
    for chunk in tiles.chunks(TILES_PER_ROW) {
        let mut line = row();
        for (i, &index) in chunk.iter().enumerate() {
            if i > 0 {
                line = line.child(SizedBox(Some(TILE_GAP), None));
            }
            line = line.child(
                glass_surface(
                    SizedBox::<PlaygroundState>(Some(TILE_SIZE), Some(TILE_SIZE)).child(Padding(
                        EdgeInsets::all(8.0),
                        text(format!("{}", index + 1)).size(16.0).color(ink),
                    )),
                )
                .tier(tier)
                .corner_radius(TILE_RADIUS)
                .material(material.clone()),
            );
        }
        grid = grid.child(line).child(SizedBox(None, Some(TILE_GAP)));
    }
    let stage = stack()
        .child(StripesView { animate })
        .child(Padding(EdgeInsets::all(16.0), grid));

    let dec = button("-", move |state: &mut PlaygroundState| {
        state
            .glass_count
            .set(count.saturating_sub(1).max(MIN_SURFACES));
    })
    .style(ButtonStyle::Secondary)
    .small();
    let inc = button("+", move |state: &mut PlaygroundState| {
        state.glass_count.set((count + 1).min(MAX_SURFACES));
    })
    .style(ButtonStyle::Secondary)
    .small();

    let cap_control = segmented_button(
        CAP_CHOICES
            .iter()
            .map(|&cap| segment(cap).label(cap.to_string())),
        [selected_cap],
        |state: &mut PlaygroundState, picked: Vec<usize>| {
            if let Some(&cap) = picked.first() {
                state.glass_cap.set(set_max_backdrops_per_frame(cap));
            }
        },
    );
    let tier_control = segmented_button(
        [GlassTier::Control, GlassTier::Bar, GlassTier::Chrome]
            .into_iter()
            .map(|t| segment(t).label(tier_name(t))),
        [tier],
        |state: &mut PlaygroundState, picked: Vec<GlassTier>| {
            if let Some(&t) = picked.first() {
                state.glass_tier.set(t);
            }
        },
    );

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(text("Glass").size(13.0).color(accent))
            .child(
                text(
                    "glass_surface tiles over moving stripes. Pick how many surfaces, the \
                     per-frame backdrop cap and the tier; each tile records one backdrop blur.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(stage)
            .child(SizedBox(None, Some(12.0)))
            .child(
                row()
                    .child(text("Surfaces").size(12.0))
                    .child(SizedBox(Some(12.0), None))
                    .child(dec)
                    .child(SizedBox(Some(8.0), None))
                    .child(text(count.to_string()).size(12.0))
                    .child(SizedBox(Some(8.0), None))
                    .child(inc),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(text("Backdrop cap").size(12.0))
            .child(cap_control)
            .child(SizedBox(None, Some(8.0)))
            .child(text("Tier").size(12.0))
            .child(tier_control)
            .child(SizedBox(None, Some(8.0)))
            .child(text(format!("Applied cap: {applied_cap}")).size(12.0))
            .child(text(cap_status(count, applied_cap)).size(11.0).color(muted)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tier_material_carries_a_real_blur_and_one_wash() {
        for tier in [GlassTier::Control, GlassTier::Bar, GlassTier::Chrome] {
            let material = glass_material(tier);
            assert!(!material.is_opaque(), "{tier:?} must record a blur");
            assert_eq!(material.fills_light.len(), 1);
            assert_eq!(material.fills_dark.len(), 1);
            assert_eq!(material.hairline_alpha, 0.3);
        }
    }

    #[test]
    fn refusal_starts_one_past_the_cap() {
        assert!(!is_refused(4, 4));
        assert!(is_refused(5, 4));
        assert!(!is_refused(8, 8));
        assert!(cap_status(5, 4).contains("refused"));
        assert!(!cap_status(4, 4).contains("refused"));
    }

    #[test]
    fn stripe_phase_wraps_within_one_period() {
        for secs in [0.0, 0.3, 1.0, 17.9, 1.0e6] {
            let phase = stripe_phase(secs);
            assert!(
                (0.0..2.0 * STRIPE_WIDTH).contains(&phase),
                "{secs}: {phase}"
            );
        }
    }

    #[test]
    fn cap_choices_stay_inside_the_engine_range() {
        for cap in CAP_CHOICES {
            assert!((1..=16).contains(&cap));
        }
    }
}
