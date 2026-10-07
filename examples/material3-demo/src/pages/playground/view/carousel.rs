//! Carousel: the reference's `CarouselPlayground`.
//!
//! Every [`frust_material::carousel()`] layout knob the reference exercises is
//! ported — type, hero alignment, axis, extended, and the focal-title overlay
//! it drives off `onChange` — over the six bundled demo images.
//!
//! # The images are decoded off the UI thread, once
//!
//! `assets/i1..i6.png` are `include_bytes!`-embedded and handed to
//! [`frust::decode_image_async`] through [`frust::use_task`], the facade's
//! documented composition for exactly this (`image_async`'s own module docs).
//! Six 960×480 PNGs is ~11 MB of RGBA: the synchronous
//! [`frust::ImageSource::decode`] path `examples/huddle` uses for its single
//! small logo would spend that decode on the UI thread the frame this page
//! opens. The task's [`frust::AsyncValue`] state lives in the page's own
//! retained state, so the preview shows a one-line status until it resolves
//! and the decoded [`frust::ImageSource`] handles are re-passed (an `Arc`
//! bump, never a re-decode) on every later rebuild.
//!
//! That is also why the knobs are their own struct: [`Knobs`] is everything
//! the preview varies with and is constructible in a host test, while
//! [`PageState`] adds the [`frust::UseTask`] handle — which needs the
//! process's reactive runtime — so [`content`] stays testable across every
//! knob state with plain [`frust::ImageSource`]s handed in.
//!
//! # Descoped: `Free scroll`
//!
//! The reference's `Free scroll` switch drives `M3ECarousel.freeScroll`.
//! [`mod@frust_material::carousel`]'s own module docs list it under *Deliberately
//! not ported* (upstream's own default is off; with it on the widget swaps in
//! Flutter's ballistic scroll machinery) — snapping is the only mode this
//! catalog has. The control is omitted rather than wired to a prop that does
//! not exist.
//!
//! # Divergence: the focal title's scrim is a solid band
//!
//! The reference paints the title over a top-to-bottom `LinearGradient`
//! (transparent → 50% black). [`frust::container`] takes a solid `fill` and
//! no gradient seam (the catalog's one gradient surface is
//! `frust_material`'s button-only `ButtonDecoration`), so the title sits on a
//! single translucent band of the gradient's own end color instead.

use frust::authoring::text::{FontWeight, TextAlign};
use frust::{
    AnyView, AsyncValue, Color, Component, CrossAxisAlignment, EdgeInsets, Get, Image,
    ImageDecodeError, ImageFit, ImageSource, Padding, SizedBox, UseTask, View, any, column,
    component, container, decode_image_async, stack, text, use_task,
};
use frust_material::{
    CarouselAxis, CarouselChange, CarouselLayout, HeroAlignment, OverlayAnchor, carousel,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel,
    play_enum_segmented, play_preview_card, play_snippet, play_switch, playground_body,
};

/// The six bundled demo images and their titles — the reference's own
/// `_images` list (`assets/i1..i6.png`).
const IMAGES: [(&[u8], &str); 6] = [
    (include_bytes!("../../../../assets/i1.png"), "Android"),
    (include_bytes!("../../../../assets/i2.png"), "iOS"),
    (include_bytes!("../../../../assets/i3.png"), "Windows"),
    (include_bytes!("../../../../assets/i4.png"), "Mac"),
    (include_bytes!("../../../../assets/i5.png"), "Linux"),
    (include_bytes!("../../../../assets/i6.png"), "Others"),
];

/// Every [`CarouselLayout`] the "Type" menu offers — the reference's
/// `M3ECarouselType.values` order.
const LAYOUTS: [CarouselLayout; 3] = [
    CarouselLayout::Hero,
    CarouselLayout::Contained,
    CarouselLayout::Uncontained,
];

/// Every [`HeroAlignment`] the "Hero alignment" menu offers.
const ALIGNMENTS: [HeroAlignment; 3] = [
    HeroAlignment::Start,
    HeroAlignment::Center,
    HeroAlignment::End,
];

/// Both [`CarouselAxis`] values the "Axis" segmented control offers.
const AXES: [CarouselAxis; 2] = [CarouselAxis::Horizontal, CarouselAxis::Vertical];

/// Preview height for a horizontal carousel, in logical px — the reference's
/// own `SizedBox(height: 200)`.
const HORIZONTAL_HEIGHT: f64 = 200.0;
/// Preview height for a vertical carousel (the reference's `320`).
const VERTICAL_HEIGHT: f64 = 320.0;
/// Preview width for a vertical carousel (the reference's `200`).
const VERTICAL_WIDTH: f64 = 200.0;

/// The focal title's scrim — the end color of the reference's own
/// transparent→`0x80000000` gradient (see the module docs' divergence note).
const TITLE_SCRIM: Color = Color::from_rgba8(0x00, 0x00, 0x00, 0x80);
/// The focal title's ink and metrics — the reference's own
/// `TextStyle(color: 0xFFFFFFFF, fontSize: 18, fontWeight: w500)`.
const TITLE_INK: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
const TITLE_SIZE: f32 = 18.0;
/// Padding around the focal title — the reference's `EdgeInsets.all(12)`.
const TITLE_PADDING: f64 = 12.0;

/// Menu/snippet label — the reference's `M3ECarouselType.name`.
fn layout_label(layout: CarouselLayout) -> &'static str {
    match layout {
        CarouselLayout::Hero => "hero",
        CarouselLayout::Contained => "contained",
        CarouselLayout::Uncontained => "uncontained",
    }
}

/// Menu/snippet label. Upstream names these for the screen (`left`/`right`);
/// [`HeroAlignment`] names them for the scroll axis, and so does this label.
fn alignment_label(alignment: HeroAlignment) -> &'static str {
    match alignment {
        HeroAlignment::Start => "start",
        HeroAlignment::Center => "center",
        HeroAlignment::End => "end",
    }
}

/// Segmented-control/snippet label — the reference's `Axis.name`.
fn axis_label(axis: CarouselAxis) -> &'static str {
    match axis {
        CarouselAxis::Horizontal => "horizontal",
        CarouselAxis::Vertical => "vertical",
    }
}

/// Everything the preview varies with — see the module docs for why this is
/// separate from [`PageState`].
struct Knobs {
    layout: CarouselLayout,
    alignment: HeroAlignment,
    axis: CarouselAxis,
    extended: bool,
    show_titles: bool,
    /// The focal item the carousel last reported through `on_change` — the
    /// reference's own `_focalIndex`.
    focal: usize,
    /// Shared with [`layout_menu_panel`] — the "Type" dropdown's anchor.
    layout_anchor: OverlayAnchor,
    layout_open: bool,
    /// Shared with [`alignment_menu_panel`] — the "Hero alignment" anchor.
    alignment_anchor: OverlayAnchor,
    alignment_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_CarouselPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            layout: CarouselLayout::Hero,
            alignment: HeroAlignment::Center,
            axis: CarouselAxis::Horizontal,
            extended: false,
            show_titles: true,
            focal: 1,
            layout_anchor: OverlayAnchor::new(),
            layout_open: false,
            alignment_anchor: OverlayAnchor::new(),
            alignment_open: false,
        }
    }
}

/// This page's retained state: the knobs plus the off-thread image decode.
struct PageState {
    knobs: Knobs,
    images: UseTask<Vec<ImageSource>>,
}

/// Decode all six bundled images off the UI thread, in order.
async fn decode_demo_images() -> Result<Vec<ImageSource>, ImageDecodeError> {
    let mut decoded = Vec::with_capacity(IMAGES.len());
    for (bytes, _) in IMAGES {
        decoded.push(decode_image_async(bytes.to_vec()).await?);
    }
    Ok(decoded)
}

struct CarouselPlayground;

impl Component for CarouselPlayground {
    type State = PageState;

    fn init(&self) -> PageState {
        PageState {
            knobs: Knobs::default(),
            images: use_task(decode_demo_images),
        }
    }

    fn build(&self, state: &mut PageState) -> impl View<PageState> {
        let decoded = state.images.signal().get();
        match &decoded {
            AsyncValue::Ready(images) => content(&state.knobs, images, None),
            AsyncValue::Error(error) => content(
                &state.knobs,
                &[],
                Some(&format!("Image decode failed: {error}")),
            ),
            AsyncValue::Idle | AsyncValue::Loading(_) => {
                content(&state.knobs, &[], Some("Decoding images…"))
            }
        }
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(CarouselPlayground))
}

/// The page body: the playground content plus the two dropdown panels it
/// anchors, stacked so both paint above the scrollable content.
///
/// `status` replaces the carousel with a one-line message while the images
/// are not decoded yet (see the module docs).
fn content(
    knobs: &Knobs,
    images: &[ImageSource],
    status: Option<&str>,
) -> impl View<PageState> + use<> {
    let preview = match status {
        Some(message) => any(status_view(message)),
        None => any(carousel_preview(knobs, images)),
    };
    let body = playground_body(
        vec![any(play_preview_card("Carousel", preview))],
        vec![snippet(knobs)],
        vec![controls(knobs)],
    );
    stack()
        .child(body)
        .child(layout_menu_panel(knobs))
        .child(alignment_menu_panel(knobs))
}

/// The decode's non-ready state, shown in the preview card's slot.
fn status_view(message: &str) -> impl View<PageState> {
    let theme = ambient_theme();
    let mut style = theme.type_scale.body_medium.clone();
    style.color = theme.scheme().on_surface_variant;
    text(message.to_string()).style(style)
}

/// The carousel itself, in the reference's own fixed preview box.
///
/// No counterpart to the reference's `ValueKey('$_type-$_axis-...')`: it
/// re-keys the widget so a layout change rebuilds it from scratch, and
/// [`mod@frust_material::carousel`]'s own `rebuild` already re-slots and re-seats
/// its offset when any of those props change (its "jumps back" note, ported
/// from `M3ECarousel.didUpdateWidget`).
fn carousel_preview(knobs: &Knobs, images: &[ImageSource]) -> impl View<PageState> {
    let vertical = knobs.axis == CarouselAxis::Vertical;
    let items: Vec<AnyView<PageState>> = images
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let title = IMAGES.get(index).map(|(_, title)| *title).unwrap_or("");
            carousel_item(
                source.clone(),
                title,
                knobs.show_titles && index == knobs.focal,
            )
        })
        .collect();

    let view = carousel(items)
        .layout(knobs.layout)
        .hero_alignment(knobs.alignment)
        .extended(knobs.extended)
        .axis(knobs.axis)
        .on_change(|state: &mut PageState, change: CarouselChange| {
            state.knobs.focal = change.focal_index;
        });

    let (width, height) = if vertical {
        (Some(VERTICAL_WIDTH), Some(VERTICAL_HEIGHT))
    } else {
        (None, Some(HORIZONTAL_HEIGHT))
    };
    SizedBox::<PageState>(width, height).child(view)
}

/// One carousel cell: the image, plus the focal title band over it — the
/// reference's `_CarouselImage`.
fn carousel_item(source: ImageSource, title: &str, show_title: bool) -> AnyView<PageState> {
    let image = any(Image(source).fit(ImageFit::Cover));
    if !show_title {
        return image;
    }
    any(stack().child(image).child(title_band(title)))
}

/// The bottom-pinned title band — see the module docs' scrim divergence.
fn title_band(title: &str) -> impl View<PageState> {
    let theme = ambient_theme();
    let mut style = theme.type_scale.title_medium.clone();
    style.size = TITLE_SIZE;
    style.weight = FontWeight::MEDIUM;
    style.color = TITLE_INK;
    style.align = TextAlign::Center;

    let band = container(Padding(
        EdgeInsets::all(TITLE_PADDING),
        text(title.to_string()).style(style),
    ))
    .fill(TITLE_SCRIM);

    column()
        .flex(1, SizedBox::<PageState>(None, None))
        .child(band)
        .cross_axis(CrossAxisAlignment::Stretch)
}

fn controls(knobs: &Knobs) -> AnyView<PageState> {
    control_panel::<PageState>(
        "Layout",
        vec![
            play_enum_menu_field::<PageState, CarouselLayout>(
                "Type",
                knobs.layout,
                &LAYOUTS,
                layout_label,
                &knobs.layout_anchor,
                knobs.layout_open,
                |state: &mut PageState, open: bool| state.knobs.layout_open = open,
            ),
            play_enum_segmented::<PageState, CarouselAxis>(
                "Axis",
                knobs.axis,
                &AXES,
                axis_label,
                |state: &mut PageState, next: CarouselAxis| state.knobs.axis = next,
            ),
            play_enum_menu_field::<PageState, HeroAlignment>(
                "Hero alignment",
                knobs.alignment,
                &ALIGNMENTS,
                alignment_label,
                &knobs.alignment_anchor,
                knobs.alignment_open,
                |state: &mut PageState, open: bool| state.knobs.alignment_open = open,
            ),
            play_switch::<PageState>(
                "Extended",
                knobs.extended,
                |state: &mut PageState, next: bool| state.knobs.extended = next,
            ),
            play_switch::<PageState>(
                "Show titles",
                knobs.show_titles,
                |state: &mut PageState, next: bool| state.knobs.show_titles = next,
            ),
        ],
    )
}

/// The "Type" menu's popup half — mounted at this page's outer [`Stack`].
fn layout_menu_panel(knobs: &Knobs) -> impl View<PageState> {
    play_enum_menu_panel::<PageState, CarouselLayout>(
        knobs.layout,
        &LAYOUTS,
        layout_label,
        &knobs.layout_anchor,
        knobs.layout_open,
        |state: &mut PageState, open: bool| state.knobs.layout_open = open,
        |state: &mut PageState, next: CarouselLayout| state.knobs.layout = next,
    )
}

/// The "Hero alignment" menu's popup half — mounted at the outer [`Stack`].
fn alignment_menu_panel(knobs: &Knobs) -> impl View<PageState> {
    play_enum_menu_panel::<PageState, HeroAlignment>(
        knobs.alignment,
        &ALIGNMENTS,
        alignment_label,
        &knobs.alignment_anchor,
        knobs.alignment_open,
        |state: &mut PageState, open: bool| state.knobs.alignment_open = open,
        |state: &mut PageState, next: HeroAlignment| state.knobs.alignment = next,
    )
}

fn snippet(knobs: &Knobs) -> PlaySnippet {
    play_snippet(
        "Carousel",
        format!(
            "carousel(items)\n\
             \u{20}   .layout(CarouselLayout::{layout:?})\n\
             \u{20}   .hero_alignment(HeroAlignment::{alignment:?})\n\
             \u{20}   .axis(CarouselAxis::{axis:?})\n\
             \u{20}   .extended({extended})\n\
             \u{20}   .on_change(|state, change| {{\n\
             \u{20}       state.focal = change.focal_index;\n\
             \u{20}   }});",
            layout = knobs.layout,
            alignment = knobs.alignment,
            axis = knobs.axis,
            extended = knobs.extended,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-ins for the decoded assets: the page never inspects a source's
    /// pixels, only re-passes the handle.
    fn fake_images(count: usize) -> Vec<ImageSource> {
        (0..count)
            .map(|_| ImageSource::from_rgba8(vec![0, 0, 0, 255], 1, 1))
            .collect()
    }

    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let images = fake_images(IMAGES.len());
        let mut knobs = Knobs::default();
        for layout in LAYOUTS {
            for alignment in ALIGNMENTS {
                for axis in AXES {
                    for extended in [true, false] {
                        knobs.layout = layout;
                        knobs.alignment = alignment;
                        knobs.axis = axis;
                        knobs.extended = extended;
                        let _view = content(&knobs, &images, None);
                    }
                }
            }
        }

        knobs = Knobs::default();
        knobs.show_titles = false;
        let _view = content(&knobs, &images, None);

        knobs.show_titles = true;
        knobs.focal = IMAGES.len() - 1;
        let _view = content(&knobs, &images, None);

        knobs.layout_open = true;
        knobs.alignment_open = true;
        let _view = content(&knobs, &images, None);
    }

    #[test]
    fn the_page_builds_in_both_undecoded_states() {
        let knobs = Knobs::default();
        let _loading = content(&knobs, &[], Some("Decoding images…"));
        let _failed = content(&knobs, &[], Some("Image decode failed: bad bytes"));
    }

    #[test]
    fn the_page_builds_with_fewer_images_than_titles() {
        let knobs = Knobs::default();
        let _view = content(&knobs, &fake_images(2), None);
    }

    #[test]
    fn the_snippet_tracks_the_layout_knobs() {
        let mut knobs = Knobs::default();
        assert!(snippet(&knobs).code.contains("CarouselLayout::Hero"));
        assert!(snippet(&knobs).code.contains("HeroAlignment::Center"));
        knobs.layout = CarouselLayout::Uncontained;
        knobs.axis = CarouselAxis::Vertical;
        knobs.extended = true;
        let code = snippet(&knobs).code;
        assert!(code.contains("CarouselLayout::Uncontained"));
        assert!(code.contains("CarouselAxis::Vertical"));
        assert!(code.contains(".extended(true)"));
    }
}
