//! Driving-family tools: finding widgets and acting on them.
//!
//! # One round trip per call
//!
//! `find_widgets` (and a `tap` targeted by query) issues **exactly one**
//! `widget_tree` request and does all flattening and filtering here. This is
//! the binding lesson carried over from the fdemon-pro driver: a tool that
//! re-fetches, retries, or polls the app to "settle" a query turns one agent
//! step into a burst of device round trips and, on a device, into seconds of
//! latency the agent then times out on. If a caller wants the tree after
//! something changed, it calls the tool again — that decision is the agent's,
//! never this layer's.
//!
//! Coordinates throughout are **logical px**: the same space `WidgetNode`'s
//! bounds arrive in and the same space the app's own input path uses, so a
//! reported `center` is directly tappable with no conversion
//! (`docs/CODE_STANDARDS.md`'s Interaction Semantics).

use frust_devtools_protocol::{WidgetNode, WidgetTreeDump};
use rmcp::handler::server::wrapper::Json;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::{Point, ToolError, ToolResult, WidgetMatch, resolve_session, with_devtools};
use crate::engine::SessionEngine;

/// Default cap on how many matches `find_widgets` returns — a full tree is
/// unbounded and an agent picking a target reads the first handful.
const DEFAULT_FIND_LIMIT: usize = 25;

/// Hard cap on `find_widgets`'s `limit`.
const MAX_FIND_LIMIT: usize = 200;

/// How many candidates an ambiguity error lists before it stops — enough to
/// disambiguate by hand, bounded so a query matching hundreds of nodes does
/// not answer with the whole tree.
const MAX_CANDIDATES: usize = 20;

// ── Arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindWidgetsArgs {
    /// Case-insensitive substring of the widget's type name (e.g. `Button`).
    #[serde(default)]
    pub type_name: Option<String>,
    /// Case-insensitive substring of the widget's debug label (e.g. `Save`).
    #[serde(default)]
    pub label: Option<String>,
    /// Maximum matches to return (default 25).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TapArgs {
    /// Logical-px x. Give x and y together, or a widget query instead.
    #[serde(default)]
    pub x: Option<f64>,
    /// Logical-px y.
    #[serde(default)]
    pub y: Option<f64>,
    /// Case-insensitive substring of the target's type name.
    #[serde(default)]
    pub type_name: Option<String>,
    /// Case-insensitive substring of the target's debug label.
    #[serde(default)]
    pub label: Option<String>,
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ScrollArgs {
    /// Logical-px x the scroll originates at.
    pub x: f64,
    /// Logical-px y the scroll originates at.
    pub y: f64,
    /// Horizontal delta in logical px.
    pub dx: f64,
    /// Vertical delta in logical px (positive scrolls content up, like a
    /// wheel-down).
    pub dy: f64,
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EnterTextArgs {
    /// The text to type into whatever currently has focus.
    pub text: String,
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WidgetPropsArgs {
    /// A widget id from find_widgets or widget_tree.
    pub id: u64,
    /// Defaults to the only running session; required when several are.
    #[serde(default)]
    pub session_id: Option<u64>,
}

// ── Results ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct FindWidgetsResult {
    pub session_id: u64,
    /// How many widgets matched in the whole tree.
    pub total_matches: usize,
    /// How many nodes the tree held (matched or not).
    pub tree_nodes: usize,
    /// Whether `widgets` was cut short by `limit`.
    pub truncated: bool,
    pub widgets: Vec<WidgetMatch>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct TapResult {
    pub session_id: u64,
    /// The logical-px point actually tapped.
    pub tapped: Point,
    /// The widget the query resolved to; absent for a coordinate tap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widget: Option<WidgetMatch>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ScrollResult {
    pub session_id: u64,
    pub at: Point,
    pub dx: f64,
    pub dy: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct EnterTextResult {
    pub session_id: u64,
    /// How many characters were sent (the text itself is not echoed back).
    pub characters: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct WidgetPropsResult {
    pub session_id: u64,
    pub id: u64,
    /// The widget's debug properties, as name/value pairs.
    pub properties: Vec<PropertyDto>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct PropertyDto {
    pub name: String,
    pub value: String,
}

// ── Tools ───────────────────────────────────────────────────────────────────

pub(crate) async fn find_widgets(
    engine: &SessionEngine,
    args: FindWidgetsArgs,
) -> ToolResult<FindWidgetsResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let dump = match with_devtools(engine, &snapshot, "widget_tree", |client| {
        client.widget_tree()
    })
    .await
    {
        Ok(dump) => dump,
        Err(err) => return Err(Json(err)),
    };

    let limit = args.limit.unwrap_or(DEFAULT_FIND_LIMIT).min(MAX_FIND_LIMIT);
    let query = Query::new(args.type_name.as_deref(), args.label.as_deref());
    let found = query.search(&dump);
    Ok(Json(FindWidgetsResult {
        session_id: snapshot.id.0,
        total_matches: found.matches.len(),
        tree_nodes: found.visited,
        truncated: found.matches.len() > limit,
        widgets: found.matches.into_iter().take(limit).collect(),
    }))
}

pub(crate) async fn tap(engine: &SessionEngine, args: TapArgs) -> ToolResult<TapResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let has_query = args.type_name.is_some() || args.label.is_some();
    let point = match (args.x, args.y, has_query) {
        (Some(_), Some(_), true) => {
            return Err(Json(ToolError::new(
                "give coordinates (x and y) or a widget query (type_name and/or label), \
                 not both.",
            )));
        }
        (Some(x), Some(y), false) => Target::Point(Point { x, y }),
        (None, None, true) => {
            Target::Query(Query::new(args.type_name.as_deref(), args.label.as_deref()))
        }
        (None, None, false) => {
            return Err(Json(ToolError::new(
                "nothing to tap: pass x and y, or a widget query (type_name and/or label). \
                 find_widgets lists what is on screen.",
            )));
        }
        _ => {
            return Err(Json(ToolError::new(
                "x and y must be given together — a single coordinate is not a point.",
            )));
        }
    };

    let (point, widget) = match point {
        Target::Point(point) => (point, None),
        Target::Query(query) => {
            let dump = match with_devtools(engine, &snapshot, "widget_tree", |client| {
                client.widget_tree()
            })
            .await
            {
                Ok(dump) => dump,
                Err(err) => return Err(Json(err)),
            };
            match query.resolve_one(&dump) {
                Ok(widget) => {
                    let center = widget.center.expect("a tappable widget has a center");
                    (center, Some(widget))
                }
                Err(err) => return Err(Json(err)),
            }
        }
    };

    let (x, y) = (point.x, point.y);
    if let Err(err) = with_devtools(engine, &snapshot, "tap", move |client| {
        client.tap(x, y).map(|_| ())
    })
    .await
    {
        return Err(Json(err));
    }
    Ok(Json(TapResult {
        session_id: snapshot.id.0,
        tapped: point,
        widget,
    }))
}

pub(crate) async fn scroll(engine: &SessionEngine, args: ScrollArgs) -> ToolResult<ScrollResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let ScrollArgs { x, y, dx, dy, .. } = args;
    if let Err(err) = with_devtools(engine, &snapshot, "scroll", move |client| {
        client.scroll(x, y, dx, dy).map(|_| ())
    })
    .await
    {
        return Err(Json(err));
    }
    Ok(Json(ScrollResult {
        session_id: snapshot.id.0,
        at: Point { x, y },
        dx,
        dy,
    }))
}

pub(crate) async fn enter_text(
    engine: &SessionEngine,
    args: EnterTextArgs,
) -> ToolResult<EnterTextResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let characters = args.text.chars().count();
    let text = args.text;
    if let Err(err) = with_devtools(engine, &snapshot, "enter_text", move |client| {
        client.text(text).map(|_| ())
    })
    .await
    {
        return Err(Json(err));
    }
    Ok(Json(EnterTextResult {
        session_id: snapshot.id.0,
        characters,
    }))
}

pub(crate) async fn widget_props(
    engine: &SessionEngine,
    args: WidgetPropsArgs,
) -> ToolResult<WidgetPropsResult> {
    let snapshot = match resolve_session(engine, args.session_id) {
        Ok(snapshot) => snapshot,
        Err(err) => return Err(Json(err)),
    };
    let id = args.id;
    let props = match with_devtools(engine, &snapshot, "widget_props", move |client| {
        client.widget_props(id)
    })
    .await
    {
        Ok(props) => props,
        Err(err) => return Err(Json(err)),
    };
    Ok(Json(WidgetPropsResult {
        session_id: snapshot.id.0,
        id: props.id,
        properties: props
            .entries
            .into_iter()
            .map(|(name, value)| PropertyDto { name, value })
            .collect(),
    }))
}

// ── Matching ────────────────────────────────────────────────────────────────

/// What a `tap` resolved its arguments to.
enum Target {
    Point(Point),
    Query(Query),
}

/// A widget query: case-insensitive substring filters, all of which must
/// match. An empty query matches every node.
pub(crate) struct Query {
    type_name: Option<String>,
    label: Option<String>,
}

/// The outcome of walking a tree once.
pub(crate) struct Found {
    pub matches: Vec<WidgetMatch>,
    /// How many nodes the walk visited — reported so an agent whose query
    /// matched nothing can tell an empty tree from a bad filter.
    pub visited: usize,
}

impl Query {
    pub(crate) fn new(type_name: Option<&str>, label: Option<&str>) -> Self {
        Self {
            type_name: type_name.map(str::to_lowercase),
            label: label.map(str::to_lowercase),
        }
    }

    fn is_empty(&self) -> bool {
        self.type_name.is_none() && self.label.is_none()
    }

    fn describe(&self) -> String {
        match (&self.type_name, &self.label) {
            (Some(type_name), Some(label)) => {
                format!("type_name {type_name:?} and label {label:?}")
            }
            (Some(type_name), None) => format!("type_name {type_name:?}"),
            (None, Some(label)) => format!("label {label:?}"),
            (None, None) => "no filters".to_string(),
        }
    }

    fn matches(&self, node: &WidgetNode) -> bool {
        let type_ok = self
            .type_name
            .as_deref()
            .is_none_or(|needle| node.type_name.to_lowercase().contains(needle));
        let label_ok = self.label.as_deref().is_none_or(|needle| {
            node.debug_label
                .as_deref()
                .is_some_and(|label| label.to_lowercase().contains(needle))
        });
        type_ok && label_ok
    }

    /// Walks the whole dump **once**, depth-first, collecting matches in
    /// document order.
    pub(crate) fn search(&self, dump: &WidgetTreeDump) -> Found {
        let mut found = Found {
            matches: Vec::new(),
            visited: 0,
        };
        for root in &dump.roots {
            self.walk(root, &mut found);
        }
        found
    }

    fn walk(&self, node: &WidgetNode, found: &mut Found) {
        found.visited += 1;
        if self.matches(node) {
            found.matches.push(WidgetMatch::from_node(node));
        }
        for child in &node.children {
            self.walk(child, found);
        }
    }

    /// Resolves this query to the one tappable widget it names, or an error
    /// that lists what it could have meant.
    pub(crate) fn resolve_one(&self, dump: &WidgetTreeDump) -> Result<WidgetMatch, ToolError> {
        if self.is_empty() {
            return Err(ToolError::new(
                "a widget query needs type_name and/or label.",
            ));
        }
        let found = self.search(dump);
        let (tappable, untappable): (Vec<WidgetMatch>, Vec<WidgetMatch>) = found
            .matches
            .into_iter()
            .partition(WidgetMatch::is_tappable);

        match tappable.len() {
            1 => Ok(tappable.into_iter().next().expect("length checked")),
            0 if untappable.is_empty() => Err(ToolError::new(format!(
                "no widget matched {} (searched {} nodes). Call find_widgets to see what is \
                 on screen.",
                self.describe(),
                found.visited
            ))),
            0 => Err(ToolError::new(format!(
                "{} widget(s) matched {} but none has a non-empty on-screen rect, so a tap \
                 would hit nothing. They may not be laid out or visible yet.",
                untappable.len(),
                self.describe()
            ))
            .with_candidates(untappable.into_iter().take(MAX_CANDIDATES).collect())),
            n => Err(ToolError::new(format!(
                "{n} widgets matched {} — narrow the query, or tap the center coordinates of \
                 the one you want.",
                self.describe()
            ))
            .with_candidates(tappable.into_iter().take(MAX_CANDIDATES).collect())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_devtools_protocol::RectPx;

    fn rect(x: f64, y: f64) -> Option<RectPx> {
        Some(RectPx {
            x,
            y,
            width: 20.0,
            height: 10.0,
        })
    }

    fn node(
        id: u64,
        type_name: &str,
        label: Option<&str>,
        bounds: Option<RectPx>,
        children: Vec<WidgetNode>,
    ) -> WidgetNode {
        WidgetNode {
            id,
            type_name: type_name.to_string(),
            debug_label: label.map(str::to_string),
            bounds,
            children,
        }
    }

    /// Root
    /// ├─ ButtonWidget "Save"   (tappable)
    /// ├─ ButtonWidget "Cancel" (tappable)
    /// └─ TextWidget   "Save"   (no bounds)
    fn tree() -> WidgetTreeDump {
        WidgetTreeDump {
            roots: vec![node(
                1,
                "RootWidget",
                None,
                rect(0.0, 0.0),
                vec![
                    node(
                        2,
                        "ButtonWidget",
                        Some("Save"),
                        rect(10.0, 20.0),
                        Vec::new(),
                    ),
                    node(
                        3,
                        "ButtonWidget",
                        Some("Cancel"),
                        rect(40.0, 20.0),
                        Vec::new(),
                    ),
                    node(4, "TextWidget", Some("Save"), None, Vec::new()),
                ],
            )],
        }
    }

    #[test]
    fn a_query_filters_by_substring_case_insensitively() {
        let found = Query::new(Some("button"), None).search(&tree());
        assert_eq!(found.visited, 4);
        assert_eq!(
            found.matches.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn type_name_and_label_filters_are_anded() {
        let found = Query::new(Some("Button"), Some("save")).search(&tree());
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].id, 2);
        let center = found.matches[0].center.expect("center");
        assert_eq!((center.x, center.y), (20.0, 25.0));
    }

    #[test]
    fn an_empty_query_matches_every_node_in_document_order() {
        let found = Query::new(None, None).search(&tree());
        assert_eq!(
            found.matches.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn resolving_an_ambiguous_query_lists_the_candidates() {
        let err = Query::new(Some("Button"), None)
            .resolve_one(&tree())
            .expect_err("two buttons match");
        assert!(err.error.contains("2 widgets matched"), "{}", err.error);
        assert_eq!(
            err.candidates.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn a_match_with_no_rect_is_reported_as_untappable_not_as_a_target() {
        let err = Query::new(Some("Text"), None)
            .resolve_one(&tree())
            .expect_err("the text node has no bounds");
        assert!(err.error.contains("none has a non-empty"), "{}", err.error);
        assert_eq!(err.candidates.len(), 1);
        assert_eq!(err.candidates[0].id, 4);
    }

    #[test]
    fn a_query_matching_nothing_says_how_much_was_searched() {
        let err = Query::new(Some("Slider"), None)
            .resolve_one(&tree())
            .expect_err("no sliders");
        assert!(err.error.contains("searched 4 nodes"), "{}", err.error);
        assert!(err.candidates.is_empty());
    }

    #[test]
    fn an_unambiguous_query_resolves_to_its_center() {
        let widget = Query::new(None, Some("cancel"))
            .resolve_one(&tree())
            .expect("one match");
        assert_eq!(widget.id, 3);
        let center = widget.center.expect("center");
        assert_eq!((center.x, center.y), (50.0, 25.0));
    }
}
