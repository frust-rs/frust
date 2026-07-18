//! Activity tab — the mentions feed derived from [`mock::activity`].
//!
//! Hosts a [`features::activity::ActivityController`] (via `use_controller`,
//! the same seam [`crate::screens::settings_appearance`] uses) behind a
//! `Component`, since the feed's async load state (skeletons → rows) and
//! per-row unread flags must survive rebuilds — a plain view-fn's local
//! variables reset every frame (see `docs/CODE_STANDARDS.md`'s State &
//! Reactivity Conventions).
//!
//! # Navigation (a documented, deliberate simplification)
//!
//! `src/routes.rs`'s `/activity` entry calls [`activity_screen`] with **no**
//! `NavigatorController` — unlike `/`, `/you`, `/channel/:id`, and
//! `/you/settings`, which each receive one (see `src/routes.rs`) — and
//! `src/routes.rs` is a frozen hub file this task may not edit
//! (`src/README-phase-c.md`). A `Component`-hosted screen's event handlers
//! only ever see its own local `State` (never the outer `HuddleState` a
//! `NavigatorController<HuddleState>` would route through — see
//! `docs/ARCHITECTURE.md`'s Component state boundary), so a row tap cannot
//! push the app's shared `/channel/:id` route from here.
//!
//! Instead, a tap opens a small in-tab detail view over this screen's own
//! local [`ActivityState::open`] field: the same channel content (rendered
//! from the same [`mock`] data `channel_feed` reads), reached without the
//! shared navigator. Wiring the literal shared route needs a one-line
//! `src/routes.rs` change (passing a `NavigatorController<HuddleState>` into
//! [`activity_screen`], mirroring `channel_feed`'s entry) — flagged as a
//! follow-up hub task in this task's completion notes, not smuggled in here.

use std::sync::Arc;

use clean_signals::async_state::AsyncState;
use clean_signals_forgekit::use_controller;
use forgekit::{
    AnyView, Axis, Column, CrossAxisAlignment, FlexView, GestureDetector, Get, any, app_bar,
    component, filled_card, filter_chip, flexible, hero, icon, icons, inflexible, list_item,
    scroll_view, text,
};

use crate::HuddleState;
use crate::failure::HuddleFailure;
use crate::features::activity::{ActivityController, ActivityRow};
use crate::mock::{self, ActivityItem, MessageBody};

/// The Activity tab root — see the module docs' Navigation note for why this
/// takes no `NavigatorController`.
pub fn activity_screen() -> AnyView<HuddleState> {
    any(component(ActivityScreen))
}

/// The Activity tab's `Component`. Stateless configuration; all retained
/// state lives in [`ActivityState`].
struct ActivityScreen;

/// Retained state: the hosted controller, plus the local in-tab "drill-down"
/// selection (see the module docs' Navigation note).
struct ActivityState {
    controller: Arc<ActivityController>,
    /// `Some(item)` shows that mention's channel content in place of the
    /// feed list; `None` shows the feed.
    open: Option<ActivityItem>,
}

impl forgekit::Component for ActivityScreen {
    type State = ActivityState;

    fn init(&self) -> ActivityState {
        let controller = use_controller::<ActivityController, HuddleFailure>(|| {
            ActivityController::new(mock::activity())
        });
        {
            let handle = Arc::clone(&controller);
            forgekit::spawn_local(async move {
                handle.load().await;
            });
        }
        ActivityState {
            controller,
            open: None,
        }
    }

    fn build(&self, state: &mut ActivityState) -> AnyView<ActivityState> {
        if let Some(item) = state.open.clone() {
            return detail_view(item);
        }

        // Tracked read: `ActivityController::load`/`mark_all_read` write this
        // signal from a spawned task, waking the frame (see
        // `docs/ARCHITECTURE.md`'s Signal-driven wake).
        let snapshot = state.controller.rows.get();
        let body = match snapshot {
            AsyncState::Loading => skeleton_body(),
            AsyncState::Data(rows) | AsyncState::Reloading(rows) => {
                if rows.is_empty() {
                    empty_body()
                } else {
                    list_body(rows)
                }
            }
            // `LoadActivity`/`MarkAllRead` never fail (see
            // `features::activity`'s module docs); kept exhaustive rather
            // than `unreachable!()` so a future fallible use case swap isn't
            // a silent panic trap.
            AsyncState::Error { .. } => empty_body(),
        };

        let bar = app_bar::<ActivityState>("Activity").actions(vec![any(GestureDetector(
            icon(icons::DONE_ALL).size(24.0).label("Mark all read"),
        )
        .on_tap(|s: &mut ActivityState| {
            let handle = Arc::clone(&s.controller);
            forgekit::spawn_local(async move {
                handle.mark_all_read().await;
            });
        }))]);

        any(FlexView::new(
            Axis::Vertical,
            vec![inflexible(any(bar)), flexible(1, any(scroll_view(body)))],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }
}

/// Loading skeletons: three blank placeholder rows at the feed's normal row
/// height, shown while [`ActivityController::rows`] is
/// [`AsyncState::Loading`].
fn skeleton_body() -> AnyView<ActivityState> {
    any(Column(
        (0..3)
            .map(|_| any(list_item::<ActivityState>("Loading…").supporting(" ")))
            .collect(),
    ))
}

/// The "nothing to see" empty state, shown when the feed resolved with zero
/// items.
fn empty_body() -> AnyView<ActivityState> {
    any(Column(vec![
        any(icon(icons::CHECK).size(48.0)),
        any(text("You're all caught up").size(16.0)),
    ]))
}

/// The loaded feed: one row per [`ActivityRow`].
fn list_body(rows: Vec<ActivityRow>) -> AnyView<ActivityState> {
    any(Column(rows.into_iter().map(row_view).collect()))
}

/// A synthetic, presentation-only "relative time" label.
///
/// `mock::ActivityItem` carries no real timestamp field (`src/mock/**` is a
/// frozen hub file, and ships none — see `src/README-phase-c.md`), so this
/// derives a stable, deterministic label from the item's `message_id` alone.
/// A mock stand-in for a real elapsed-time computation, not one itself.
fn relative_time(message_id: u32) -> String {
    let hours = (message_id % 23) + 1;
    format!("{hours}h ago")
}

/// One feed row: actor avatar (hero-wrapped, tag `avatar-{user_id}`), bold
/// actor + action text, message excerpt + relative time, and — while unread —
/// an "Unread" indicator chip. Tapping the row opens [`detail_view`] for its
/// channel (see the module docs' Navigation note).
fn row_view(row: ActivityRow) -> AnyView<ActivityState> {
    let ActivityRow { item, unread } = row;

    let actor = mock::user(item.author_id);
    let actor_name = actor.map(|u| u.name).unwrap_or("Someone");
    let initials = actor.map(|u| u.initials).unwrap_or("?");
    let channel_name = mock::channel(item.channel_id)
        .map(|c| c.name.to_string())
        .unwrap_or_else(|| item.channel_id.to_string());

    // The whole headline is bold (actor + action text) — `Text` has no
    // mixed-weight run support, so this bolds the full line rather than just
    // the actor's name (see `docs/CODE_STANDARDS.md`'s theming/token notes on
    // documented simplifications).
    let headline = format!("{actor_name} mentioned you in #{channel_name}");
    let supporting = format!("{} · {}", item.text, relative_time(item.message_id));

    let avatar = hero(
        format!("avatar-{}", item.author_id),
        filled_card(text(initials.to_string()).size(14.0)),
    );

    let mut list_row = list_item::<ActivityState>(headline)
        .supporting(supporting)
        .three_line()
        .leading(avatar);

    if unread {
        // A static "Unread" indicator (the dot/tint the spec calls for) — a
        // real `filter_chip` rather than a hand-rolled fill, since production
        // Huddle code depends on the `forgekit` facade alone, not
        // `forgekit-core` directly (see `examples/huddle/Cargo.toml`'s
        // dependency notes), which rules out a custom leaf `Widget` here. The
        // chip's own toggle callback is a deliberate no-op: it is a passive
        // per-row marker, not itself interactive (the app bar's "mark all
        // read" action is the one control that clears it).
        list_row = list_row.trailing(filter_chip::<ActivityState, _>(
            "Unread",
            true,
            |_s: &mut ActivityState, _selected: bool| {},
        ));
    }

    let target = item;
    any(list_row.on_press(move |s: &mut ActivityState| {
        s.open = Some(target);
    }))
}

/// The in-tab channel "detail" view a row tap opens (see the module docs'
/// Navigation note) — the mentioned channel's messages, with a back action
/// that clears [`ActivityState::open`].
fn detail_view(item: ActivityItem) -> AnyView<ActivityState> {
    let title = mock::channel(item.channel_id)
        .map(|c| format!("#{}", c.name))
        .unwrap_or_else(|| item.channel_id.to_string());

    let rows: Vec<AnyView<ActivityState>> = mock::messages_for(item.channel_id)
        .into_iter()
        .map(|m| {
            let author = mock::user(m.author_id).map(|u| u.name).unwrap_or("Someone");
            let excerpt = match m.body {
                MessageBody::Text(t) => t.to_string(),
                MessageBody::Link {
                    title: link_title, ..
                } => format!("\u{1F517} {link_title}"),
                MessageBody::File { name, .. } => format!("\u{1F4CE} {name}"),
            };
            any(list_item::<ActivityState>(author.to_string()).supporting(excerpt))
        })
        .collect();

    let bar = app_bar::<ActivityState>(title).leading(any(GestureDetector(
        icon(icons::ARROW_BACK).size(24.0).label("Back"),
    )
    .on_tap(|s: &mut ActivityState| {
        s.open = None;
    })));

    any(FlexView::new(
        Axis::Vertical,
        vec![
            inflexible(any(bar)),
            flexible(1, any(scroll_view(Column(rows)))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}
