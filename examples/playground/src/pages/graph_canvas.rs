//! Graph section: a toy node-and-edge graph demonstrating two widgets added
//! earlier in this plan — `CanvasView` (a-01, `frust::canvas`) painting the
//! graph's edges and node circles in local space, hosted inside a
//! `PanZoomView` (a-09, `frust::pan_zoom`) viewport that drags to pan and
//! ctrl/⌘+wheels or pinches to zoom.
//!
//! # Layout
//!
//! [`NODE_COUNT`] nodes sit at fixed content-space positions ([`node_positions`],
//! a deterministic spiral so the graph reads as a real layout rather than a
//! grid) inside a content area ([`CONTENT_SIZE`]) deliberately larger than any
//! realistic viewport — [`PanZoomView`](frust::PanZoomView)'s own doc note: "a
//! child with no natural size is laid out at the viewport size", so the graph
//! canvas is given an explicit [`CanvasView::size`](frust::CanvasView::size)
//! to give the pan/zoom view real content to pan around inside.
//!
//! # Why labels aren't painted inside the canvas
//!
//! A [`CanvasView`](frust::CanvasView) paint closure only carries
//! `&PaintCtx`, not a `LayoutCtx` — so it has no seam to reach the shared
//! `frust-text::TextContext` every other page shapes text through (at layout
//! time, via `LayoutCtx::text_context`, the established pattern e.g.
//! `frust_material::navigation_rail`'s own `TextRun` carries). Rather than
//! hand-rolling a second, ad hoc text-shaping path for just this canvas, each
//! node's label is an ordinary [`text`] view, positioned at the node's exact
//! content-space coordinate with [`Padding`]'s left/top insets (a `Stack`
//! always places its children at its own origin — `crate::pages` doesn't
//! import `Stack` for any other reason) — so the label still pans/zooms in
//! lock-step with the canvas underneath it, since both live inside the same
//! `PanZoomView` child.
//!
//! # Interaction
//!
//! [`CanvasView::on_hit`] narrows hit-testing to "inside some node's circle"
//! ([`hit_node`]); a hit-admitted `Down` (delivered through
//! [`CanvasView::on_pointer`], since [`CanvasView::on_tap`] carries no
//! position) selects the nearest node, highlighted on the next paint via
//! [`CanvasView::repaint_key`]. A press that misses every node falls through
//! to the `PanZoomView`, which then pans instead — the same "offer the Down to
//! the child first" contract `pan_zoom`'s own module docs describe.
//!
//! The "Fit" button drives the attached
//! [`PanZoomController`](frust::PanZoomController) — the same handle
//! [`PlaygroundState::graph_transform`]'s readout reads back through
//! `.on_transform`.

use frust::authoring::{PaintCtx, PaintScene, PointerEvent, PointerPhase};
use frust::kurbo::{Point, Size, Vec2};
use frust::{
    AnyView, ButtonStyle, Color, EdgeInsets, Get, Padding, PanZoomTransform, Set, SizedBox, Stack,
    Theme, View, any, button, canvas, column, pan_zoom, row, text, use_context,
};

use crate::PlaygroundState;

/// How many toy nodes the graph shows.
const NODE_COUNT: usize = 12;

/// The graph's content-space extent — deliberately larger than any realistic
/// viewport so there is real room to pan around in (see the [module
/// docs](self)).
const CONTENT_SIZE: Size = Size::new(1400.0, 1000.0);

/// The bounded height the pan/zoom viewport gets inside the page's own
/// scrolling column (the shell wraps every page body in a `scroll_view` —
/// see `pages`' page-fn contract — so an un-sized `PanZoomView` here would
/// read an unbounded height and collapse onto its own content instead of
/// giving the user a fixed window to pan within).
const VIEWPORT_H: f64 = 460.0;

/// Each node's painted (and tappable) radius, in content-space px.
const NODE_RADIUS: f64 = 20.0;

/// The golden angle, in radians (`\u{3c0}(3 - \u{221a}5)`) — spaces the spiral's
/// nodes with no two ever landing on the same ray from the center, the same
/// angle phyllotaxis spirals use for even spread.
const GOLDEN_ANGLE_RAD: f64 = 2.399_963_229_728_653;

/// The toy graph's edges, as node-index pairs: a spanning tree over all
/// [`NODE_COUNT`] nodes plus two extra links for a couple of cycles —
/// arbitrary content for the canvas/pan-zoom demo, not a meaningful graph.
const EDGES: [(usize, usize); 13] = [
    (0, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (0, 5),
    (1, 6),
    (2, 7),
    (3, 8),
    (4, 9),
    (5, 10),
    (5, 11),
    (6, 7),
    (8, 9),
];

/// Every node's content-space center, node 0 at the content area's center and
/// every other node spiraling out from it at the golden angle — deterministic
/// (not random), so the layout is identical across runs and platforms.
fn node_positions() -> [Point; NODE_COUNT] {
    let center = Point::new(CONTENT_SIZE.width / 2.0, CONTENT_SIZE.height / 2.0);
    let mut nodes = [center; NODE_COUNT];
    for (index, node) in nodes.iter_mut().enumerate().skip(1) {
        let angle = index as f64 * GOLDEN_ANGLE_RAD;
        let radius = 90.0 + index as f64 * 32.0;
        *node = center + Vec2::new(radius * angle.cos(), radius * angle.sin());
    }
    nodes
}

/// `"N<index>"` — the node's label, also its accessible/debug name.
fn node_label(index: usize) -> String {
    format!("N{index}")
}

/// The node whose circle contains `point` (content-space, [`NODE_RADIUS`]
/// from its center), or `None` when `point` falls outside every node — the
/// shared test [`CanvasView::on_hit`]/[`CanvasView::on_pointer`] both apply
/// (see the [module docs](self)).
fn hit_node(point: Point, nodes: &[Point; NODE_COUNT]) -> Option<usize> {
    nodes
        .iter()
        .position(|&center| (point - center).hypot() <= NODE_RADIUS)
}

/// Paint every edge (as a line between its two nodes' centers) then every
/// node (a filled circle, highlighted when `selected`) — in that order, so
/// node fills paint over the edge lines meeting them.
fn paint_graph(
    nodes: [Point; NODE_COUNT],
    selected: Option<usize>,
    edge_ink: Color,
    node_fill: Color,
    selected_fill: Color,
) -> impl Fn(&mut dyn PaintScene, Size, &PaintCtx) + 'static {
    move |scene, _size, _ctx| {
        for &(a, b) in &EDGES {
            scene.stroke_line(nodes[a], nodes[b], 2.0, edge_ink);
        }
        for (index, &center) in nodes.iter().enumerate() {
            let fill = if Some(index) == selected {
                selected_fill
            } else {
                node_fill
            };
            let origin = Point::new(center.x - NODE_RADIUS, center.y - NODE_RADIUS);
            scene.fill_rounded_rect(
                origin,
                Size::new(NODE_RADIUS * 2.0, NODE_RADIUS * 2.0),
                NODE_RADIUS,
                fill,
            );
        }
    }
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let selected = state.graph_selected.get();
    let transform = state.graph_transform.get();
    let nodes = node_positions();

    let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
    let scheme = theme.scheme();
    let accent = scheme.primary;
    let muted = scheme.on_surface_variant;
    let edge_ink = scheme.outline_variant;
    let node_fill = scheme.secondary_container;
    let selected_fill = scheme.primary;

    let canvas_view = canvas(paint_graph(
        nodes,
        selected,
        edge_ink,
        node_fill,
        selected_fill,
    ))
    .size(CONTENT_SIZE)
    .on_hit(move |point: Point, _size: Size| hit_node(point, &nodes).is_some())
    .on_pointer(move |state: &mut PlaygroundState, event: PointerEvent| {
        if event.phase == PointerPhase::Down
            && let Some(index) = hit_node(event.position, &nodes)
        {
            state.graph_selected.set(Some(index));
        }
    })
    .repaint_key(selected);

    let mut layers: Vec<AnyView<PlaygroundState>> = Vec::with_capacity(NODE_COUNT + 1);
    layers.push(any(canvas_view));
    for (index, &center) in nodes.iter().enumerate() {
        let left = (center.x - NODE_RADIUS).max(0.0);
        let top = (center.y + NODE_RADIUS + 4.0).max(0.0);
        layers.push(any(Padding(
            EdgeInsets {
                left,
                top,
                right: 0.0,
                bottom: 0.0,
            },
            text(node_label(index)).size(11.0),
        )));
    }
    let world = Stack(layers);

    let graph_viewport = SizedBox::<PlaygroundState>(None, Some(VIEWPORT_H)).child(
        pan_zoom(world)
            .min_scale(0.25)
            .max_scale(4.0)
            .inertia(true)
            .controller(state.graph_controller.clone())
            .on_transform(|state: &mut PlaygroundState, transform: PanZoomTransform| {
                state.graph_transform.set(transform);
            }),
    );

    let selected_label = selected
        .map(node_label)
        .unwrap_or_else(|| "none".to_string());
    let readout = format!(
        "scale {:.2}x  offset ({:.0}, {:.0})  selected: {selected_label}",
        transform.scale, transform.offset.x, transform.offset.y,
    );
    let controller = state.graph_controller.clone();
    let fit_button = button("Fit", move |_state: &mut PlaygroundState| {
        controller.fit_to_bounds();
    })
    .style(ButtonStyle::Secondary)
    .small();

    Padding(
        EdgeInsets::all(16.0),
        column()
            .child(text("Graph").size(13.0).color(accent))
            .child(
                text(
                    "A toy node-and-edge graph: CanvasView (a-01) paints nodes/edges inside a \
                         PanZoomView (a-09) viewport. Drag to pan, ctrl/\u{2318}+wheel or pinch to \
                         zoom, tap a node to select it, “Fit” frames the whole graph.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(12.0)))
            .child(graph_viewport)
            .child(SizedBox(None, Some(8.0)))
            .child(
                row()
                    .child(fit_button)
                    .child(SizedBox(Some(12.0), None))
                    .child(text(readout).size(12.0)),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_positions_cover_every_node_and_stay_in_content_bounds() {
        let nodes = node_positions();
        assert_eq!(nodes.len(), NODE_COUNT);
        for node in nodes {
            assert!(
                node.x >= NODE_RADIUS
                    && node.x <= CONTENT_SIZE.width - NODE_RADIUS
                    && node.y >= NODE_RADIUS
                    && node.y <= CONTENT_SIZE.height - NODE_RADIUS,
                "node {node:?} must stay within the content bounds (minus its own radius)"
            );
        }
    }

    #[test]
    fn edges_reference_only_valid_node_indices() {
        for &(a, b) in &EDGES {
            assert!(
                a < NODE_COUNT && b < NODE_COUNT,
                "edge ({a}, {b}) out of range"
            );
        }
    }

    #[test]
    fn hit_node_finds_the_node_under_its_own_center_and_nothing_far_away() {
        let nodes = node_positions();
        for (index, &center) in nodes.iter().enumerate() {
            assert_eq!(hit_node(center, &nodes), Some(index));
        }
        // Far outside the content area entirely: no node claims it.
        assert_eq!(hit_node(Point::new(-1000.0, -1000.0), &nodes), None);
    }

    #[test]
    fn node_label_formats_plainly() {
        assert_eq!(node_label(0), "N0");
        assert_eq!(node_label(11), "N11");
    }
}
