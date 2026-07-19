//! Search tab — live filter across channels, messages, and people.
//!
//! The search field ([`frust::TextInput`], filtering as-you-type via
//! `on_change`) drives
//! [`SearchController`](crate::features::search::SearchController)'s query
//! signal; its `Memo`-derived results render as three sections (Channels /
//! People / Messages), each row navigating: a channel or message row to its
//! `/channel/:id`, a person row to `/user/:id` through a `hero`-wrapped
//! avatar tagged `avatar-{user_id}` (the shared cross-screen hero-tag
//! convention future roster/profile screens can reuse). An empty query shows
//! a search hint; a non-empty, non-matching query shows a dedicated
//! no-results view. See `crate::features::search`'s module docs for why the
//! controller is a thread-local singleton rather than `Component`-hosted
//! state (this screen needs `&mut HuddleState` in its row callbacks to
//! navigate, which a `Component`'s inner state boundary would block).
//!
//! # Fixed outer shape, keyed inner content
//!
//! The outer tree is a fixed two-child `Column` — `[search_field,
//! results_container]` — instead of one `Column` whose child count changes
//! per keystroke. This is defense-in-depth for the focus-preservation
//! reconciler fix (`docs/ARCHITECTURE.md`'s Event pipeline / the
//! `12-huddle-search-structure` task): even though the framework now
//! preserves an unchanged sibling's focus/IME state across a structural
//! rebuild, the search field never needs that fallback here because its
//! slot's own child count is invariant. [`results_container`] is the single
//! stable slot at index 1 whose content swaps internally (hint / no-results /
//! result rows) as the query changes; every child inside it — including the
//! single hint/no-results placeholder — carries a stable key via
//! [`keyed`], so a query transition never mixes keyed and unkeyed siblings
//! in the same list (see `docs/CODE_STANDARDS.md`'s keyed-list contract:
//! keys are all-or-nothing per list).
//!
//! [`search_field`] applies the identical "stable shape, swap content
//! internally" pattern to itself, one level down (`12b-search-field-stable-row`):
//! its own row is always the same `FlexView` wrapping the field plus a clear
//! button, rather than being a bare `TextInput` while the query is empty and
//! swapping to the `FlexView` wrapper the moment it isn't — a fix for a
//! sibling defect the `12-huddle-search-structure` task's own completion
//! summary first surfaced (see that task's Risks item 1): a genuine `AnyView`
//! type change at `Padding`'s single `ChildPod` on that transition dropped
//! the field's recorded focus/IME path, silently swallowing the very next
//! keystroke on the real per-frame pipeline.

use frust::{
    Align, Alignment, AnyView, Axis, Color, Column, CrossAxisAlignment, EdgeInsets, FlexChild,
    FlexView, GestureDetector, Get, Padding, Set, SizedBox, TextInput, any, filled_card, flexible,
    hero, icon, icons, inflexible, keyed, list_item, scroll_view, text,
};

use crate::HuddleState;
use crate::features::search::{MessageHit, SearchController, SearchResults};
use crate::mock;
use crate::screens::scaffold;

/// Forced height of the search field row: a `SizedBox`, not the field's own
/// font-metric-dependent natural height, so the layout (and anything
/// asserting against it) stays independent of text shaping.
const FIELD_HEIGHT: f64 = 56.0;
/// Forced height of a section header row — see [`FIELD_HEIGHT`]'s doc.
const SECTION_HEADER_HEIGHT: f64 = 32.0;
/// Fixed avatar badge size (leading slot of a person row).
const AVATAR_SIZE: f64 = 40.0;

/// The Search tab root: a fixed two-child outer `Column` — `[search_field,
/// results_container]` — so a query edit only ever changes content INSIDE
/// `results_container`, never the outer child count (see the module docs).
pub fn search_screen() -> AnyView<HuddleState> {
    let controller = SearchController::instance();
    let query = controller.query.get();
    let results = controller.results.get();

    let children: Vec<AnyView<HuddleState>> = vec![
        search_field(controller, &query),
        results_container(&query, results),
    ];

    scaffold("Search", any(scroll_view(Column(children))))
}

/// The single stable child of the outer `Column` (always index 1): its own
/// content swaps internally between the empty-query hint, the no-results
/// view, and the keyed section rows as the query changes (see the module
/// docs). Every branch keys its child(ren) so a transition between branches
/// never mixes keyed and unkeyed siblings in the same list (all-or-nothing
/// per `docs/CODE_STANDARDS.md`'s keyed-list contract).
fn results_container(query: &str, results: SearchResults) -> AnyView<HuddleState> {
    let children: Vec<FlexChild<HuddleState>> = if query.trim().is_empty() {
        vec![keyed("hint", hint_view())]
    } else if results.is_empty() {
        vec![keyed("no-results", no_results_view(query))]
    } else {
        keyed_section_views(results)
    };

    any(FlexView::new(Axis::Vertical, children))
}

/// The search field row: a controlled [`TextInput`] filtering as-you-type,
/// plus a clear (`icons::CLOSE`) button, ALWAYS rendered as the same
/// `FlexView(Horizontal, [flexible(field), inflexible(clear_icon)])` shape —
/// stable shape, swap content internally, the same pattern
/// [`results_container`] applies to its own content (see the module docs).
///
/// The clear-button slot's own concrete type never changes either: rather
/// than being present only once the query is non-empty (the pre-fix
/// behavior), it always renders the identical `GestureDetector`-wrapped
/// [`IconView`](frust::IconView), painted fully transparent
/// ([`Color::TRANSPARENT`]) and wired to a no-op tap while the query is
/// empty — an inert placeholder of the SAME widget, not a swapped-in
/// different one. Before this fix, `row`'s own concrete type flipped between
/// a bare `TextInput` (empty query) and this `FlexView` wrapper (non-empty)
/// — a genuine `AnyView` type change at `Padding`'s single `ChildPod`, which
/// tears down and rebuilds the child, dropping the `TextInput`'s recorded
/// focus/IME path the instant the query crossed the empty/non-empty boundary
/// (silently swallowing the very next keystroke on the real per-frame
/// mobile/desktop pipeline — see `12b-search-field-stable-row`'s task file
/// for the full mechanism, first surfaced as a Risk in
/// `12-huddle-search-structure`'s completion summary).
fn search_field(controller: SearchController, query: &str) -> AnyView<HuddleState> {
    let field = TextInput::<HuddleState, _>(
        query.to_string(),
        move |_s: &mut HuddleState, text: String| {
            controller.query.set(text);
        },
    )
    .placeholder("Search channels, people, messages");

    let has_query = !query.is_empty();
    let mut clear_icon = icon(icons::CLOSE).size(20.0);
    if !has_query {
        // Invisible placeholder: same `IconView`, painted transparent instead
        // of resolving the theme's default icon color.
        clear_icon = clear_icon.color(Color::TRANSPARENT);
    }

    let row: AnyView<HuddleState> = any(FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(1, any(field)),
            inflexible(any(GestureDetector(clear_icon).on_tap(
                move |_s: &mut HuddleState| {
                    // No-op while the query is already empty — the placeholder
                    // is inert, not just invisible.
                    if has_query {
                        controller.query.set(String::new());
                    }
                },
            ))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center));

    any(SizedBox::<HuddleState>(None, Some(FIELD_HEIGHT))
        .child(Padding(EdgeInsets::symmetric(16.0, 0.0), row)))
}

/// The empty-query hint state: `icons::SEARCH` plus a short instruction.
fn hint_view() -> AnyView<HuddleState> {
    any(Padding(
        EdgeInsets::all(32.0),
        Column(vec![
            any(Align(Alignment::CENTER, icon(icons::SEARCH).size(48.0))),
            any(SizedBox::<HuddleState>(None, Some(12.0))),
            any(text("Search channels, people, messages").size(14.0)),
        ]),
    ))
}

/// The no-results state for a non-empty, non-matching `query`.
fn no_results_view(query: &str) -> AnyView<HuddleState> {
    any(Padding(
        EdgeInsets::all(32.0),
        text(format!("No results for '{query}'")).size(16.0),
    ))
}

/// The three result sections, in Channels / People / Messages order, each
/// preceded by a header — only sections with at least one hit render. Every
/// header and row is keyed by a stable id (a channel's string id, a user's
/// numeric id, a message hit's id) so a row's widget — and any internal state
/// it holds — survives across query edits that reorder or partially overlap
/// the result set, instead of being torn down and rebuilt positionally.
fn keyed_section_views(results: SearchResults) -> Vec<FlexChild<HuddleState>> {
    let mut views = Vec::new();
    if !results.channels.is_empty() {
        views.push(keyed("header-channels", section_header("Channels")));
        views.extend(
            results
                .channels
                .into_iter()
                .map(|c| keyed(format!("channel-{}", c.id), channel_row(c))),
        );
    }
    if !results.users.is_empty() {
        views.push(keyed("header-people", section_header("People")));
        views.extend(
            results
                .users
                .into_iter()
                .map(|u| keyed(format!("user-{}", u.id), user_row(u))),
        );
    }
    if !results.messages.is_empty() {
        views.push(keyed("header-messages", section_header("Messages")));
        views.extend(
            results
                .messages
                .into_iter()
                .map(|hit| keyed(format!("message-{}", hit.message_id), message_row(hit))),
        );
    }
    views
}

fn section_header(label: &str) -> AnyView<HuddleState> {
    any(
        SizedBox::<HuddleState>(None, Some(SECTION_HEADER_HEIGHT)).child(Padding(
            EdgeInsets::symmetric(16.0, 8.0),
            text(label.to_string()).size(13.0),
        )),
    )
}

/// A channel result row — navigates to its `/channel/:id` feed.
fn channel_row(c: mock::Channel) -> AnyView<HuddleState> {
    let id = c.id.to_string();
    any(list_item::<HuddleState>(format!("#{}", c.name))
        .supporting(c.topic.to_string())
        .leading(icon(icons::TAG).size(24.0))
        .on_press(move |s: &mut HuddleState| {
            s.nav.router().push(&format!("/channel/{id}"));
        }))
}

/// A person result row — navigates to `/user/:id` through a hero-wrapped
/// avatar tagged `avatar-{id}` (the shared cross-screen hero convention).
fn user_row(u: mock::User) -> AnyView<HuddleState> {
    let id = u.id;
    any(list_item::<HuddleState>(u.name.to_string())
        .supporting(u.status.label().to_string())
        .leading(hero(format!("avatar-{id}"), avatar_badge(u.initials)))
        .on_press(move |s: &mut HuddleState| {
            s.nav.router().push(&format!("/user/{id}"));
        }))
}

/// A message result row — navigates to the message's own `/channel/:id`.
fn message_row(hit: MessageHit) -> AnyView<HuddleState> {
    let channel_id = hit.channel_id.to_string();
    let author = mock::user(hit.author_id)
        .map(|u| u.name.to_string())
        .unwrap_or_else(|| "Someone".to_string());
    any(list_item::<HuddleState>(author)
        .supporting(hit.text.to_string())
        .leading(icon(icons::FORUM).size(24.0))
        .on_press(move |s: &mut HuddleState| {
            s.nav.router().push(&format!("/channel/{channel_id}"));
        }))
}

/// A fixed-size initials badge (leading slot of a person row) — the
/// "initials-circle escape-hatch" `PLAN.md` names, approximated here with a
/// filled card (no dedicated circular-avatar primitive exists yet in the
/// widget catalog).
fn avatar_badge(initials: &str) -> impl frust::View<HuddleState> {
    SizedBox::<HuddleState>(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(filled_card(Align(
        Alignment::CENTER,
        any(text(initials.to_string()).size(14.0)),
    )))
}
