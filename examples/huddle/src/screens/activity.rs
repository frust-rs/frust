//! Activity tab — the mentions feed derived from [`mock::activity`].
//!
//! Reads a thread-local [`ActivityController`] instance
//! ([`ActivityController::instance`]) rather than hosting one behind a
//! `Component`: the feed's async load state (skeletons → rows) and per-row
//! unread flags still need to survive rebuilds (a plain view-fn's local
//! variables reset every frame — see `docs/CODE_STANDARDS.md`'s State &
//! Reactivity Conventions), but a `Component`'s inner event handlers only
//! ever see the component's own local `State`, never the outer `HuddleState`
//! a row tap needs to reach `state.nav.router().push(..)`
//! (`docs/ARCHITECTURE.md`'s Component state boundary). Keeping this screen a
//! **plain** function — the same shape [`crate::screens::search`] and
//! [`thread`](crate::features::messages::presentation::pages::thread) use, and their documented reasoning — is what
//! lets a row tap push the real `/channel/:id` route directly, instead of
//! task 14's local in-tab drill-down workaround this task replaces. See
//! [`crate::features::activity`]'s module docs for the thread-local
//! instance's own "why" (mirroring
//! [`crate::features::search::SearchController::instance`]).

use std::sync::Arc;

use clean_signals::async_state::AsyncState;
use frust::{
    Align, Alignment, AnyView, Axis, Column, CrossAxisAlignment, FlexView, GestureDetector, Get,
    SizedBox, any, app_bar, filled_card, filter_chip, flexible, hero, icon, icons, inflexible,
    list_item, scroll_view, text,
};

/// CircleAvatar diameter — radius 20 → 40 (RESEARCH.md "Material sizing
/// reference"), matching the roster's leading avatars.
const AVATAR_SIZE: f64 = 40.0;

use crate::HuddleState;
use crate::features::activity::{ActivityController, ActivityRow};
use crate::mock;

/// The Activity tab root.
pub fn activity_screen() -> AnyView<HuddleState> {
    let controller = ActivityController::instance();

    // Tracked read: `ActivityController::load`/`mark_all_read` write this
    // signal from a spawned task, waking the frame (see
    // `docs/ARCHITECTURE.md`'s Signal-driven wake).
    let snapshot = controller.rows.get();
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
        // `features::activity`'s module docs); kept exhaustive rather than
        // `unreachable!()` so a future fallible use case swap isn't a silent
        // panic trap.
        AsyncState::Error { .. } => empty_body(),
    };

    let mark_all_read_ctrl = Arc::clone(&controller);
    let bar = app_bar::<HuddleState>("Activity").actions(vec![any(GestureDetector(
        icon(icons::DONE_ALL).size(24.0).label("Mark all read"),
    )
    .on_tap(move |_s: &mut HuddleState| {
        let handle = Arc::clone(&mark_all_read_ctrl);
        frust::spawn_local(async move {
            handle.mark_all_read().await;
        });
    }))]);

    any(FlexView::new(
        Axis::Vertical,
        vec![inflexible(any(bar)), flexible(1, any(scroll_view(body)))],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}

/// Loading skeletons: three blank placeholder rows at the feed's normal row
/// height, shown while [`ActivityController::rows`] is
/// [`AsyncState::Loading`].
fn skeleton_body() -> AnyView<HuddleState> {
    any(Column(
        (0..3)
            .map(|_| any(list_item::<HuddleState>("Loading…").supporting(" ")))
            .collect(),
    ))
}

/// The "nothing to see" empty state, shown when the feed resolved with zero
/// items.
fn empty_body() -> AnyView<HuddleState> {
    any(Column(vec![
        any(icon(icons::CHECK).size(48.0)),
        any(text("You're all caught up").size(16.0)),
    ]))
}

/// The loaded feed: one row per [`ActivityRow`].
fn list_body(rows: Vec<ActivityRow>) -> AnyView<HuddleState> {
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
/// an "Unread" indicator chip. Tapping the row navigates to the mentioned
/// channel's real `/channel/:id` feed.
fn row_view(row: ActivityRow) -> AnyView<HuddleState> {
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

    // A 40px card-backed avatar (the roster metric), initials centered with the
    // SizedBox+Align idiom — the same shape `screens::search::avatar_badge` uses.
    let avatar = hero(
        format!("avatar-{}", item.author_id),
        SizedBox(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(filled_card(Align(
            Alignment::CENTER,
            any(text(initials.to_string()).size(14.0)),
        ))),
    );

    let mut list_row = list_item::<HuddleState>(headline)
        .supporting(supporting)
        .three_line()
        .leading(avatar);

    if unread {
        // A static "Unread" indicator (the dot/tint the spec calls for) — a
        // real `filter_chip` rather than a hand-rolled fill, since production
        // Huddle code depends on the `frust` facade alone, not
        // `frust-core` directly (see `examples/huddle/Cargo.toml`'s
        // dependency notes), which rules out a custom leaf `Widget` here. The
        // chip's own toggle callback is a deliberate no-op: it is a passive
        // per-row marker, not itself interactive (the app bar's "mark all
        // read" action is the one control that clears it).
        list_row = list_row.trailing(filter_chip::<HuddleState, _>(
            "Unread",
            true,
            |_s: &mut HuddleState, _selected: bool| {},
        ));
    }

    let channel_id = item.channel_id.to_string();
    any(list_row.on_press(move |s: &mut HuddleState| {
        s.nav.router().push(&format!("/channel/{channel_id}"));
    }))
}
