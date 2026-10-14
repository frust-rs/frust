//! Images section: the `frust-network-image` plugin's device-gate page.
//!
//! Six `network_image` views in a two-column grid, each with a skeleton placeholder, an
//! error icon, `ImageFit::Cover`, a 400x300 decode target and a 250 ms fade-in.
//!
//! - **Remote** (default): `https://picsum.photos/seed/frust-<n>/800/600`. These need a
//!   network; in airplane mode every cell shows its error slot.
//! - **Use local fixtures**: swaps the URLs for a loopback server the page starts on first
//!   use, serving PNGs generated in memory (`images_server.rs`). Works offline.
//! - **Stats row**: the shared loader's memory-cache occupancy, disk-cache occupancy
//!   (`ImageLoader::disk_stats`, `None` until the first miss opens the cache) and in-flight
//!   load count, refreshed every frame.
//! - **Clear caches**: `ImageLoader::clear_all` empties the memory cache and the on-disk
//!   entries; relaunching afterwards proves a cold start.
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
    button, checkbox, colored_box, column, component, container, icon, icons, row, text,
    use_context, views,
};
use frust_network_image::{ImageLoader, NetworkImages, network_image};

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
                button("Clear caches", |_state: &mut ImagesState| {
                    if let Some(loader) = NetworkImages::loader() {
                        loader.clear_all();
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
        let cells = (0..COLUMNS).fold(row(), |cells, col| {
            let index = line * COLUMNS + col;
            let cells = if col > 0 {
                cells.child(SizedBox(Some(GAP), None))
            } else {
                cells
            };
            cells.flex(1, Self::cell(index, local_base))
        });
        column().child(cells).child(SizedBox(None, Some(GAP)))
    }

    fn cell(index: usize, local_base: Option<&str>) -> impl View<ImagesState> {
        SizedBox(None, Some(CELL_HEIGHT)).child(
            network_image::<ImagesState>(Self::url(index, local_base))
                .placeholder(Self::skeleton())
                .error(Self::error_slot())
                .fit(ImageFit::Cover)
                .target_px(400, 300)
                .fade(Duration::from_millis(250))
                .semantic_label(format!("Sample image {}", index + 1)),
        )
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

/// The cache readout (memory, disk, in-flight), rebuilt every frame by [`FrameTicker`].
struct StatsRow;

impl Component for StatsRow {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let (memory, disk) = match NetworkImages::loader() {
            Some(loader) => (Self::memory_line(&loader), Self::disk_line(&loader)),
            None => (
                "memory: no loader installed".to_string(),
                "disk: no loader installed".to_string(),
            ),
        };
        column()
            .child(text(memory).size(12.0))
            .child(text(disk).size(12.0).color(ImagesPage::muted()))
            .child(FrameTicker)
    }
}

impl StatsRow {
    fn memory_line(loader: &ImageLoader) -> String {
        let stats = loader.memory_stats();
        format!(
            "memory: {} images, {} / {} KiB decoded; in-flight: {}",
            stats.entries,
            stats.used_bytes / 1024,
            stats.budget_bytes / 1024,
            loader.inflight_count()
        )
    }

    fn disk_line(loader: &ImageLoader) -> String {
        match loader.disk_stats() {
            Some(stats) => format!(
                "disk: {} files, {} / {} KiB",
                stats.entries,
                stats.total_bytes / 1024,
                stats.cap_bytes / 1024
            ),
            None => "disk: not opened yet (opens on the first cache miss)".to_string(),
        }
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
    use frust::{AnyView, any};
    use frust_core::RenderRoot;
    use frust_reactive::ReactiveRuntime;
    use frust_text::TextContext;
    use kurbo::Size;
    use reactive_graph::owner::Owner;
    use std::any::Any;

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

    /// Every grid cell gets a real width: an unbounded-main-axis layout would give the
    /// image view a 0-width tight constraint and the engine would skip the draw.
    #[test]
    fn grid_cells_are_laid_out_with_a_real_width() {
        let _ = ReactiveRuntime::init(std::sync::Arc::new(|| {}));
        let owner = Owner::new();
        owner.set();
        let mut tcx = TextContext::new();
        let mut root: RenderRoot<PlaygroundState, AnyView<PlaygroundState>> = RenderRoot::new();
        let mut state = PlaygroundState::new();
        // erasure: keep the headless test root is typed `AnyView`; the page is erased once at this boundary
        let mut logic = |s: &mut PlaygroundState| any(page(s));
        root.rebuild(&mut logic, &mut state);
        let tcx_any: &mut dyn Any = &mut tcx;
        root.layout_with_text(Size::new(390.0, 1400.0), tcx_any);
        let update = root.semantics();
        let cells: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.label().is_some_and(|l| l.starts_with("Sample image ")))
            .collect();
        assert_eq!(cells.len(), FIXTURE_COUNT, "cells: {cells:?}");
        for (_, node) in cells {
            let rect = node.bounds().expect("cell has bounds");
            assert!(
                rect.width() > 100.0 && rect.height() > 100.0,
                "{:?} is {} x {}",
                node.label(),
                rect.width(),
                rect.height()
            );
        }
    }
}
