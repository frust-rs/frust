//! A11y section: a fixture page for the assistive-technology device gates.
//!
//! Top to bottom, each item with a caption saying what assistive technology should announce:
//!
//! 1. a heading (`text(..).heading(1)`): "Accessibility fixtures, heading level 1";
//! 2. a labelled image (`Image(..).label(..)`): "Sample gradient, image";
//! 3. a merged cell (`semantics(..).merged(true)` with label, value and hint) over drawn
//!    content: ONE node, "Battery, 80 percent, Double tap to open details", with no separate
//!    child nodes;
//! 4. a tappable tile (`semantics(GestureDetector(..).on_tap(..)).button().label(..)`):
//!    "Increment tile, button"; activating it bumps the visible counter;
//! 5. a plain labelled group (not merged) around two buttons: the group "Two actions" and
//!    each button ("First action", "Second action") are separately reachable;
//! 6. a `hidden(true)` wrapper around a text: nothing is announced and the text is skipped;
//! 7. a status line showing the tap counter.

use frust::{
    ButtonStyle, Color, EdgeInsets, GestureDetector, Get, Image, ImageFit, ImageSource, Padding,
    RwSignal, SizedBox, Theme, Update, View, button, colored_box, column, container, row,
    semantics, text, use_context,
};

use crate::PlaygroundState;

/// Edge of the sample image, in pixels.
const IMAGE_PX: u32 = 8;

/// The "A11y" section's state, owned by [`PlaygroundState`].
#[derive(Clone)]
pub struct A11yState {
    /// How many times the tappable tile was activated.
    pub taps: RwSignal<u32>,
    /// The sample gradient, built once so the image keeps one identity.
    image: ImageSource,
}

impl A11yState {
    /// Fresh state: no taps, and the sample gradient.
    pub fn new() -> Self {
        let mut pixels = Vec::with_capacity((IMAGE_PX * IMAGE_PX * 4) as usize);
        for y in 0..IMAGE_PX {
            for x in 0..IMAGE_PX {
                pixels.extend_from_slice(&[(x * 32) as u8, (y * 32) as u8, 160, 255]);
            }
        }
        A11yState {
            taps: RwSignal::new(0),
            image: ImageSource::from_rgba8(pixels, IMAGE_PX, IMAGE_PX),
        }
    }
}

impl Default for A11yState {
    fn default() -> Self {
        Self::new()
    }
}

/// The caption line under an item: what assistive technology should announce.
fn caption(content: &str) -> impl View<PlaygroundState> + use<> {
    let muted = use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant;
    column()
        .child(
            text(format!("Announces: {content}"))
                .size(11.0)
                .color(muted),
        )
        .child(SizedBox(None, Some(16.0)))
}

/// A solid tile of `color`, `width` by `height`.
fn tile(color: Color, width: f64, height: f64) -> impl View<PlaygroundState> + use<> {
    colored_box::<PlaygroundState>()
        .fill(color)
        .radius(8.0)
        .size_centered(width, height)
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let taps = state.a11y.taps;
    let count = taps.get();

    let image = SizedBox(Some(64.0), Some(64.0)).child(
        Image(state.a11y.image.clone())
            .fit(ImageFit::Contain)
            .label("Sample gradient"),
    );

    let cell = semantics(tile(scheme.tertiary_container, 160.0, 48.0))
        .label("Battery")
        .value("80 percent")
        .hint("Double tap to open details")
        .merged(true);

    let tappable_tile = semantics(
        GestureDetector(
            container(text("Tap to count").size(13.0))
                .fill(scheme.primary_container)
                .radius(8.0)
                .size_centered(160.0, 48.0),
        )
        .on_tap(move |_state: &mut PlaygroundState| taps.update(|n| *n += 1)),
    )
    .button()
    .label("Increment tile");

    let group = semantics(
        row()
            .child(
                button("One", |_state: &mut PlaygroundState| {})
                    .style(ButtonStyle::Secondary)
                    .small(),
            )
            .child(SizedBox(Some(8.0), None))
            .child(
                button("Two", |_state: &mut PlaygroundState| {})
                    .style(ButtonStyle::Secondary)
                    .small(),
            ),
    )
    .label("Two actions");

    let hidden = semantics(text("Decorative text, hidden from assistive technology").size(12.0))
        .hidden(true);

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(text("Accessibility fixtures").size(20.0).heading(1))
            .child(caption("\"Accessibility fixtures, heading level 1\""))
            .child(image)
            .child(SizedBox(None, Some(4.0)))
            .child(caption("\"Sample gradient, image\""))
            .child(cell)
            .child(SizedBox(None, Some(4.0)))
            .child(caption(
                "\"Battery, 80 percent, Double tap to open details\" as one node; the \
                 drawn box is not a separate stop",
            ))
            .child(tappable_tile)
            .child(SizedBox(None, Some(4.0)))
            .child(caption(
                "\"Increment tile, button\"; activating it adds one to the counter below",
            ))
            .child(group)
            .child(SizedBox(None, Some(4.0)))
            .child(caption(
                "the group \"Two actions\" and, inside it, the buttons \"One\" and \"Two\" \
                 as separate stops",
            ))
            .child(hidden)
            .child(SizedBox(None, Some(4.0)))
            .child(caption("nothing: the text above is skipped"))
            .child(text(format!("Taps: {count}")).size(14.0)),
    )
}
