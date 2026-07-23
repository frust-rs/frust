//! Feedback section — status/feedback widgets from the `frust::glyph`
//! catalog: badges, removable tags, alerts, toast triggers, and the three
//! loader/placeholder shapes (`research/INVENTORY.md`'s "Section 04: Status +
//! Feedback").
//!
//! # Page-local state
//!
//! This page's file-scope contract (`c04`'s task) is `src/pages/feedback.rs`
//! only — `CatalogState` (owned by `c01`'s scaffold, `src/lib.rs`) can't grow
//! a tags field for this page's removable-tags demo. Instead the whole page
//! body is a nested [`frust::component`] with its **own** retained local
//! state (the tags `Vec` signal), mirroring `examples/huddle`'s
//! `ui::toast::ToastOverlay` shape: a small `Component` capturing just the
//! outer handle it needs (here, `CatalogState::toasts`, so a toast button can
//! append to the same [`glyph::toast_host`] queue the shell mounts) and
//! owning everything else itself. See `docs/ARCHITECTURE.md`'s Component
//! state boundary.
//!
//! # Toast variant caveat
//!
//! [`glyph::toast_host`]'s v1 contract (`crates/frust-widgets/src/glyph/
//! toast.rs`) takes a plain `Vec<String>` queue and always plays each entry
//! as a [`glyph::ToastVariant::Plain`] toast — there is no way to thread a
//! variant through the shared string queue today. The five buttons below
//! still push a distinctly-worded message per [`glyph::ToastVariant`] (so the
//! FIFO/one-at-a-time/auto-dismiss behavior is visible for each trigger), but
//! every one renders with the same accent-colored dot — a documented
//! framework-API gap, not a bug in this page.

use frust::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, EdgeInsets, FlexChild, FlexView, Get,
    Padding, Row, RwSignal, SizedBox, Update, any, button, component, glyph, keyed, text,
};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`]. Hands the shared toast queue
/// into a nested [`FeedbackPage`] component that owns everything else locally
/// (see the [module docs](self)).
pub fn page(state: &CatalogState) -> AnyView<CatalogState> {
    any(component(FeedbackPage {
        toasts: state.toasts,
    }))
}

/// Config for the nested feedback-demo component (see the [module docs](self)).
struct FeedbackPage {
    /// The shell's shared toast queue ([`CatalogState::toasts`]), captured so
    /// this page's toast buttons append to the one [`glyph::toast_host`] the
    /// shell mounts.
    toasts: RwSignal<Vec<String>>,
}

/// Retained local state: the captured toast queue handle plus the page-local
/// removable-tags list (this page's only real local state).
struct FeedbackState {
    toasts: RwSignal<Vec<String>>,
    tags: RwSignal<Vec<String>>,
}

impl Component for FeedbackPage {
    type State = FeedbackState;

    fn init(&self) -> FeedbackState {
        FeedbackState {
            toasts: self.toasts,
            tags: RwSignal::new(vec![
                "rust".to_string(),
                "gpu".to_string(),
                "reactive".to_string(),
            ]),
        }
    }

    fn build(&self, state: &mut FeedbackState) -> AnyView<FeedbackState> {
        let column = Column(vec![
            section_title("Badges"),
            v_gap(10.0),
            badges_row(),
            v_gap(28.0),
            section_title("Tags"),
            v_gap(10.0),
            tags_row(state),
            v_gap(28.0),
            section_title("Alerts"),
            v_gap(10.0),
            alerts_column(),
            v_gap(28.0),
            section_title("Toasts"),
            v_gap(10.0),
            toasts_row(state),
            v_gap(28.0),
            section_title("Loaders"),
            v_gap(10.0),
            loaders_column(),
        ])
        .cross_axis(CrossAxisAlignment::Start);

        any(Padding(EdgeInsets::all(16.0), column))
    }
}

/// A section heading label (15px, themed `on_surface` — no hardcoded color).
fn section_title<State: 'static>(label: &str) -> AnyView<State> {
    any(text(label.to_string()).size(15.0))
}

/// A vertical spacer of `height` logical px.
fn v_gap<State: 'static>(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer of `width` logical px.
fn h_gap<State: 'static>(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

// ---------------------------------------------------------------------
// Badges — Success/Warning/Error (dotted), Neutral, Accent.
// ---------------------------------------------------------------------

fn badges_row<State: 'static>() -> AnyView<State> {
    any(Row(vec![
        any(glyph::badge("connected", glyph::BadgeVariant::Success).dot(true)),
        h_gap(8.0),
        any(glyph::badge("degraded", glyph::BadgeVariant::Warning).dot(true)),
        h_gap(8.0),
        any(glyph::badge("offline", glyph::BadgeVariant::Error).dot(true)),
        h_gap(8.0),
        any(glyph::badge("read-only", glyph::BadgeVariant::Neutral)),
        h_gap(8.0),
        any(glyph::badge("v0.44.1", glyph::BadgeVariant::Accent)),
    ]))
}

// ---------------------------------------------------------------------
// Tags — removable, backed by `FeedbackState::tags`.
// ---------------------------------------------------------------------

/// Renders the live `tags` list as a keyed row (each label is its own key —
/// unique per the demo's fixed starting set — so a removal reconciles by
/// identity rather than position, per `docs/CODE_STANDARDS.md`'s Interaction
/// Semantics "keyed lists" rule).
fn tags_row(state: &FeedbackState) -> AnyView<FeedbackState> {
    let labels = state.tags.get();

    let children: Vec<FlexChild<FeedbackState>> = labels
        .iter()
        .map(|label| {
            let remove_label = label.clone();
            keyed(
                label.clone(),
                Padding(
                    EdgeInsets {
                        left: 0.0,
                        top: 0.0,
                        right: 8.0,
                        bottom: 0.0,
                    },
                    glyph::tag(label.clone()).on_remove(move |state: &mut FeedbackState| {
                        state.tags.update(|v| v.retain(|t| t != &remove_label));
                    }),
                ),
            )
        })
        .collect();

    any(FlexView::new(Axis::Horizontal, children))
}

// ---------------------------------------------------------------------
// Alerts — all 4 variants; default icon glyphs already match the reference
// build exactly (i / ✓ / ! / ×, see `glyph::alert`'s `default_icon`).
// ---------------------------------------------------------------------

fn alerts_column<State: 'static>() -> AnyView<State> {
    any(Column(vec![
        any(glyph::alert(
            glyph::AlertVariant::Info,
            "Heads up",
            "A new workspace layout is available in settings.",
        )),
        v_gap(8.0),
        any(glyph::alert(
            glyph::AlertVariant::Success,
            "Saved",
            "Your changes have been saved.",
        )),
        v_gap(8.0),
        any(glyph::alert(
            glyph::AlertVariant::Warning,
            "Low disk space",
            "Free up space soon to avoid interruptions.",
        )),
        v_gap(8.0),
        any(glyph::alert(
            glyph::AlertVariant::Error,
            "Sync failed",
            "Check your connection and try again.",
        )),
    ]))
}

// ---------------------------------------------------------------------
// Toasts — one trigger button per `ToastVariant`, all through the shared
// `glyph::toast_host` queue (see the [module docs](self) caveat).
// ---------------------------------------------------------------------

fn toasts_row(state: &FeedbackState) -> AnyView<FeedbackState> {
    let toasts = state.toasts;

    let trigger = move |message: &'static str| {
        move |_: &mut FeedbackState| {
            toasts.update(|queue| glyph::show(queue, message));
        }
    };

    any(Column(vec![
        any(
            Row(vec![
                any(button("Plain", trigger("Plain notification"))),
                h_gap(8.0),
                any(button("Info", trigger("Info: sync started"))),
                h_gap(8.0),
                any(button("Success", trigger("Success: export complete"))),
                h_gap(8.0),
                any(button("Warning", trigger("Warning: approaching quota"))),
                h_gap(8.0),
                any(button("Error", trigger("Error: upload failed"))),
            ])
            .cross_axis(CrossAxisAlignment::Center),
        ),
        v_gap(8.0),
        any(text(
            "Toasts play one at a time (FIFO) and auto-dismiss after 2.4s.",
        )
        .size(11.0)),
    ]))
}

// ---------------------------------------------------------------------
// Loaders — progress bar, skeleton block, dots loader.
// ---------------------------------------------------------------------

fn loaders_column<State: 'static>() -> AnyView<State> {
    any(Column(vec![
        any(text("Progress (65%)".to_string()).size(12.0)),
        v_gap(6.0),
        any(glyph::progress(0.65)),
        v_gap(20.0),
        any(text("Skeleton".to_string()).size(12.0)),
        v_gap(6.0),
        any(glyph::skeleton(220.0, 16.0)),
        v_gap(20.0),
        any(text("Dots loader".to_string()).size(12.0)),
        v_gap(6.0),
        any(glyph::dots_loader()),
    ]))
}
