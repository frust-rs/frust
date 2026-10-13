//! Images section: the `frust-network-image` plugin's device-gate page.
//!
//! Six `network_image` views in a two-column grid, each with a skeleton placeholder, an
//! error icon, `ImageFit::Cover`, a 400x300 decode target and a 250 ms fade-in.
//!
//! - **Remote** (default): `https://picsum.photos/seed/frust-<n>/800/600`. These need a
//!   network; in airplane mode every cell shows its error slot.
//! - **Use local fixtures**: swaps the URLs for a loopback server the page starts on first
//!   use, serving PNGs generated in memory (`images_server.rs`). Works offline.
//! - **Stats row**: the shared loader's memory cache occupancy, refreshed every frame. The
//!   loader exposes no disk statistics and no in-flight count at this revision, so the row
//!   says so rather than guessing.
//! - **Clear memory cache**: `ImageLoader::clear_memory`. Disk clearing is not exposed by
//!   the loader either; to test a cold disk cache, clear the app's storage.
//!
//! `NetworkImages::install()` runs once from `frust::app!`'s `setup` block in `lib.rs`.
//!
//! Device-gate walk: placeholder -> fade -> image; airplane mode -> error icons; relaunch
//! the app offline after a remote load and the images still appear (disk-cache hit).

use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
};
use frust::{
    ButtonStyle, Color, Component, EdgeInsets, ImageFit, Padding, RwSignal, Set, SizedBox, Theme,
    button, checkbox, colored_box, column, component, container, flexible, icon, icons, inflexible,
    row, text, use_context, views,
};
use frust_network_image::{NetworkImages, network_image};

use super::images_server::{self, FIXTURE_COUNT};
use crate::PlaygroundState;

const COLUMNS: usize = 2;
const CELL_HEIGHT: f64 = 150.0;
const GAP: f64 = 8.0;

/// See the page-fn contract in [`crate::pages`]. Reads no [`PlaygroundState`] signal.
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(ImagesPage)
}

/// The page's retained state: whether the grid points at the loopback fixtures, and the
/// fixture server's base URL once it has been started.
struct ImagesState {
    use_local: RwSignal<bool>,
    local_base: RwSignal<Option<String>>,
}

struct ImagesPage;

impl Component for ImagesPage {
    type State = ImagesState;

    fn init(&self) -> ImagesState {
        ImagesState {
            use_local: RwSignal::new(false),
            local_base: RwSignal::new(None),
        }
    }

    fn build(&self, state: &mut ImagesState) -> impl View<ImagesState> {
        use frust::Get;
        let local_base = if state.use_local.get() {
            state.local_base.get()
        } else {
            None
        };
        let using_local = local_base.is_some();

        let body = column()
            .child(text("Images").size(13.0).color(Self::accent()))
            .child(
                text(
                    "frust-network-image: placeholder -> fade -> image, error slot, memory \
                     and disk caches. Remote cells fetch picsum.photos and need a network \
                     (airplane mode shows the error icons); the toggle switches to a \
                     loopback server with generated PNGs.",
                )
                .size(11.0)
                .color(Self::muted()),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(checkbox(
                using_local,
                "Use local fixtures",
                |state: &mut ImagesState, requested: bool| {
                    // The server starts on the first request for it and keeps running.
                    let base = if requested {
                        images_server::base_url()
                    } else {
                        None
                    };
                    state.local_base.set(base);
                    state.use_local.set(requested);
                },
            ))
            .child(SizedBox(None, Some(12.0)))
            .children(views(
                (0..FIXTURE_COUNT / COLUMNS)
                    .map(|line| Self::grid_row(line, local_base.as_deref())),
            ))
            .child(SizedBox(None, Some(4.0)))
            .child(component(StatsRow))
            .child(SizedBox(None, Some(8.0)))
            .child(
                button("Clear memory cache", |_state: &mut ImagesState| {
                    if let Some(loader) = NetworkImages::loader() {
                        loader.clear_memory();
                    }
                })
                .style(ButtonStyle::Secondary)
                .small(),
            );

        Padding(EdgeInsets::all(16.0), body)
    }
}

impl ImagesPage {
    fn accent() -> Color {
        Self::theme().scheme().primary
    }

    fn muted() -> Color {
        Self::theme().scheme().on_surface_variant
    }

    fn theme() -> Theme {
        use_context::<Theme>().unwrap_or_else(frust_material::baseline)
    }

    /// The URL of cell `index`: a fixture when `local_base` is set, else picsum.
    fn url(index: usize, local_base: Option<&str>) -> String {
        match local_base {
            Some(base) => format!("{base}/img/{index}.png"),
            None => format!("https://picsum.photos/seed/frust-{index}/800/600"),
        }
    }

    fn grid_row(line: usize, local_base: Option<&str>) -> impl View<ImagesState> {
        let cells = (0..COLUMNS).map(move |col| {
            let index = line * COLUMNS + col;
            let gap = if col + 1 < COLUMNS { GAP } else { 0.0 };
            Self::cell(index, local_base, gap)
        });
        column()
            .child(row().children(views(cells)))
            .child(SizedBox(None, Some(GAP)))
    }

    fn cell(index: usize, local_base: Option<&str>, trailing_gap: f64) -> impl View<ImagesState> {
        let image = network_image::<ImagesState>(Self::url(index, local_base))
            .placeholder(Self::skeleton())
            .error(Self::error_slot())
            .fit(ImageFit::Cover)
            .target_px(400, 300)
            .fade(Duration::from_millis(250))
            .semantic_label(format!("Sample image {}", index + 1));
        row()
            .push(flexible(1, SizedBox(None, Some(CELL_HEIGHT)).child(image)))
            .push(inflexible(SizedBox(Some(trailing_gap), None)))
    }

    fn skeleton() -> impl View<ImagesState> {
        colored_box::<ImagesState>()
            .fill(Self::theme().scheme().surface_container_highest)
            .radius(8.0)
    }

    fn error_slot() -> impl View<ImagesState> {
        let theme = Self::theme();
        let scheme = theme.scheme();
        container(icon(icons::IMAGE).size(32.0).color(scheme.error))
            .fill(scheme.error_container)
            .radius(8.0)
            .size_centered(1_000.0, CELL_HEIGHT)
    }
}

/// The memory-cache readout, rebuilt every frame by [`FrameTicker`].
struct StatsRow;

impl Component for StatsRow {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let memory = match NetworkImages::loader() {
            Some(loader) => {
                let stats = loader.memory_stats();
                format!(
                    "memory: {} images, {} / {} KiB decoded",
                    stats.entries,
                    stats.used_bytes / 1024,
                    stats.budget_bytes / 1024
                )
            }
            None => "memory: no loader installed".to_string(),
        };
        column()
            .child(text(memory).size(12.0))
            .child(
                text("disk / in-flight: not exposed by the loader at this revision")
                    .size(10.0)
                    .color(ImagesPage::muted()),
            )
            .child(FrameTicker)
    }
}

/// Zero-size view whose widget requests a paced frame on every paint, so [`StatsRow`]
/// keeps re-reading the loader while it is on screen (the camera page's ticker pattern).
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

struct FrameTickerWidget;

impl Widget for FrameTickerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
        ctx.request_frame_paced();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_switch_between_picsum_and_fixtures() {
        assert_eq!(
            ImagesPage::url(3, None),
            "https://picsum.photos/seed/frust-3/800/600"
        );
        assert_eq!(
            ImagesPage::url(3, Some("http://127.0.0.1:9")),
            "http://127.0.0.1:9/img/3.png"
        );
    }
}
