//! Thread view (`/thread/:id`).
//!
//! The root message's full thread: the parent message rendered prominently
//! (full bubble + reactions row), a "N replies" divider, the replies
//! (compact bubbles, keyed list), and a reply composer at the bottom —
//! consuming [`features::messages`](crate::features::messages)' controller
//! API (`message`/`replies_for`/`reply_count`/`reply_in_thread`, built for
//! this screen in task 12 — see that module's docs).
//!
//! # Why a thread-local composer, not a `Component` (mirrors `screens::search`)
//!
//! `src/routes.rs`'s `/thread/:id` entry (a frozen hub file, see
//! `src/README-phase-c.md`) calls [`thread_screen`] with only the route
//! param, no `NavigatorController` — unlike `/channel/:id`, which gets one
//! (see `src/routes.rs`) — so, exactly like
//! [`screens::search`](crate::screens::search)'s own documented reasoning,
//! wrapping this screen in a `Component` would trap its event handlers behind
//! the component-state boundary (`docs/ARCHITECTURE.md`'s Component state
//! boundary), unable to reach `state.nav.router()` for the back action and
//! the "N replies →" round trip this screen's own tests exercise.
//! `thread_screen` therefore stays a **plain** function closing over
//! `&mut HuddleState` directly.
//!
//! Its [`MessagesController`] now comes from the shared per-channel-id
//! registry
//! ([`MessagesController::for_channel`](crate::features::messages::MessagesController::for_channel),
//! task 19) rather than a screen-local instance — see that module's docs'
//! "One shared controller per channel" section for why that closes this
//! screen's former cross-page consistency gap: a reply composed here now
//! updates the exact same signals an already-open `channel_feed` page (or a
//! later-opened one) renders from. Only the composer's live text is
//! genuinely screen-local (a reply in flight belongs to this visit to this
//! thread, not the channel as a whole): it's cached across rebuilds behind a
//! `thread_local!` map keyed by the thread's root message id — the same
//! pattern
//! [`SearchController::instance`](crate::features::search::SearchController::instance)
//! and `screens::workspace_drawer`'s `entrance_progress` use, generalized
//! from "one slot" to "one slot per thread" so navigating between two
//! different threads (or leaving and returning to the same one) reuses the
//! right composer rather than losing it or bleeding into the wrong one.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use forgekit::{
    Align, Alignment, AnyView, Axis, CrossAxisAlignment, EdgeInsets, FlexView, GestureDetector,
    Get, GetUntracked, Padding, RwSignal, Set, SizedBox, Stack, any, app_bar, elevated_card, icon,
    icons, inflexible, keyed, outlined_card, scroll_view, text, text_input,
};

use crate::HuddleState;
use crate::features::messages::{FeedBody, FeedMessage, FeedReply, MessagesController};
use crate::mock;
use crate::screens::placeholder_body;
use crate::ui::sheet::{action_menu, emoji_grid, sheet, sheet_action_row};

/// A quick, static header timestamp — mirrors `channel_feed`'s own
/// placeholder ("real color-glyph rendering, verified in wave A"; there is no
/// real per-message timestamp in the mock dataset).
const PLACEHOLDER_TIME: &str = "9:41 AM";

/// Gets (or creates) the thread-local composer text for `root_id` — see the
/// module docs' "Why a thread-local composer" section. The controller itself
/// is no longer screen-local (see
/// [`MessagesController::for_channel`](crate::features::messages::MessagesController::for_channel));
/// only the in-flight reply text is, keyed here so a different thread never
/// bleeds into this one's composer.
fn composer_for(root_id: u32) -> RwSignal<String> {
    thread_local! {
        static COMPOSERS: RefCell<HashMap<u32, RwSignal<String>>> =
            RefCell::new(HashMap::new());
    }
    COMPOSERS.with(|cell| {
        *cell
            .borrow_mut()
            .entry(root_id)
            .or_insert_with(|| RwSignal::new(String::new()))
    })
}

/// Which message-action sheet (if any) is open over the thread (task 20).
/// Screen-local, cached behind [`thread_sheet_for`] like the composer text.
#[derive(Clone, PartialEq)]
enum ThreadSheet {
    /// No sheet — the thread is interactive.
    None,
    /// The long-press context menu. `Some(id)` (the root message) offers React;
    /// a reply bubble passes `None` (replies carry no reaction model).
    Menu(Option<u32>),
    /// The emoji picker, toggling a reaction on the root message `id`.
    Emoji(u32),
}

/// Gets (or creates) the screen-local open-sheet signal for `root_id` — the
/// counterpart to [`composer_for`], keyed the same way.
fn thread_sheet_for(root_id: u32) -> RwSignal<ThreadSheet> {
    thread_local! {
        static SHEETS: RefCell<HashMap<u32, RwSignal<ThreadSheet>>> =
            RefCell::new(HashMap::new());
    }
    SHEETS.with(|cell| {
        *cell
            .borrow_mut()
            .entry(root_id)
            .or_insert_with(|| RwSignal::new(ThreadSheet::None))
    })
}

/// The thread view for `thread_id` — the route param, parsed as the root
/// message's id.
pub fn thread_screen(thread_id: String) -> AnyView<HuddleState> {
    let Some(root_id) = thread_id.parse::<u32>().ok() else {
        return missing_thread_screen(&thread_id);
    };
    let Some(root_msg) = mock::messages()
        .into_iter()
        .chain(mock::firehose_messages())
        .find(|m| m.id == root_id)
    else {
        return missing_thread_screen(&thread_id);
    };

    let title = thread_title(root_msg.channel_id);
    let controller = MessagesController::for_channel(root_msg.channel_id);
    let composer = composer_for(root_id);
    let sheet_sig = thread_sheet_for(root_id);

    if controller.loading.get() {
        return thread_page(title, placeholder_body("Loading thread\u{2026}"), None);
    }

    // Tracked read: subscribes this rebuild to the controller's `messages`
    // signal, the same one `reply_in_thread`/`toggle_reaction` write — what
    // lets a later write (composing a reply, toggling a reaction) wake this
    // page (see `docs/ARCHITECTURE.md`'s Signal-driven wake). `message`/
    // `replies_for` (this module's named consumption point, per the module
    // docs) read the very same signal untracked, so finding the root here
    // from the tracked snapshot is equivalent, just live.
    let messages = controller.feed();
    let Some(root) = messages.into_iter().find(|m| m.id == root_id) else {
        return missing_thread_screen(&thread_id);
    };

    let content = thread_content(&controller, &root, sheet_sig);
    let composer_row = composer_bar(Arc::clone(&controller), root_id, composer);

    let screen = thread_page(title, content, Some(composer_row));
    // The message-action sheet mounts in the screen's own `Stack` top layer
    // (see `crate::ui::sheet`); inert (zero-size) when nothing is open.
    let overlay = thread_sheet(&controller, root_id, sheet_sig);
    any(Stack(vec![screen, overlay]))
}

/// The open message-action sheet as a `Stack` overlay layer, or an inert
/// zero-size box when nothing is open.
fn thread_sheet(
    controller: &Arc<MessagesController>,
    root_id: u32,
    sheet_sig: RwSignal<ThreadSheet>,
) -> AnyView<HuddleState> {
    let dismiss = move |_st: &mut HuddleState| sheet_sig.set(ThreadSheet::None);
    match sheet_sig.get() {
        ThreadSheet::None => any(SizedBox(None, None)),
        ThreadSheet::Menu(react) => {
            any(sheet(action_menu(thread_menu_rows(react, sheet_sig))).on_dismiss(dismiss))
        }
        ThreadSheet::Emoji(id) => {
            let ctrl = Arc::clone(controller);
            let picker = emoji_grid(move |_st: &mut HuddleState, emoji: &'static str| {
                ctrl.toggle_reaction(id, emoji);
                sheet_sig.set(ThreadSheet::None);
            });
            let _ = root_id;
            any(sheet(picker).on_dismiss(dismiss))
        }
    }
}

/// The long-press context-menu rows. `react` is `Some(id)` for the root message
/// (offering React → the picker) and `None` for a reply bubble.
fn thread_menu_rows(
    react: Option<u32>,
    sheet_sig: RwSignal<ThreadSheet>,
) -> Vec<AnyView<HuddleState>> {
    let mut rows: Vec<AnyView<HuddleState>> = Vec::new();
    if let Some(id) = react {
        rows.push(sheet_action_row(
            icons::MOOD,
            "React",
            move |_st: &mut HuddleState| sheet_sig.set(ThreadSheet::Emoji(id)),
        ));
    }
    rows.push(sheet_action_row(
        icons::DESCRIPTION,
        "Copy",
        move |st: &mut HuddleState| {
            sheet_sig.set(ThreadSheet::None);
            st.toasts.show("Copied");
        },
    ));
    rows.push(sheet_action_row(
        icons::DELETE,
        "Delete",
        move |st: &mut HuddleState| {
            sheet_sig.set(ThreadSheet::None);
            st.toasts.show("Only admins can delete \u{2014} mock");
        },
    ));
    rows
}

/// "Thread" plus the channel/DM name, derived from the root message's channel
/// (mirrors `channel_feed::feed_app_bar`'s channel-vs-DM title derivation).
fn thread_title(channel_id: &str) -> String {
    if let Some(ch) = mock::channel(channel_id) {
        format!("Thread  \u{b7}  #{}", ch.name)
    } else if let Some(peer) = mock::dms()
        .iter()
        .find(|d| d.id == channel_id)
        .and_then(|d| mock::user(d.user_id))
    {
        format!("Thread  \u{b7}  {}", peer.name)
    } else {
        "Thread".to_string()
    }
}

/// The app bar: a back button (pops via `state.nav`, the frozen route's only
/// navigation surface — see the module docs) plus the title.
fn thread_app_bar(title: String) -> AnyView<HuddleState> {
    let back = GestureDetector(icon(icons::ARROW_BACK).size(24.0))
        .on_tap(|s: &mut HuddleState| s.nav.router().pop());
    any(app_bar::<HuddleState>(title).leading(any(Padding(EdgeInsets::symmetric(4.0, 0.0), back))))
}

/// Assembles the app bar, scrollable `content`, and an optional `composer`
/// bar into the screen's vertical frame.
fn thread_page(
    title: String,
    content: AnyView<HuddleState>,
    composer: Option<AnyView<HuddleState>>,
) -> AnyView<HuddleState> {
    let mut children: Vec<forgekit::FlexChild<HuddleState>> = vec![
        inflexible(thread_app_bar(title)),
        forgekit::flexible(1, content),
    ];
    if let Some(composer) = composer {
        children.push(inflexible(composer));
    }
    any(FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch))
}

/// The fallback for an unresolvable route param (not a number, or no such
/// message id in the mock dataset) — mirrors `screens::profile`'s
/// `unknown_user_screen` fallback.
fn missing_thread_screen(thread_id: &str) -> AnyView<HuddleState> {
    thread_page(
        "Thread".to_string(),
        placeholder_body(&format!("No such thread: {thread_id}")),
        None,
    )
}

/// The loaded body: the root bubble, then either the empty-thread state or
/// the "N replies" divider plus the keyed reply list. The reply list is its
/// own nested `FlexView` rather than flattened into this one — keyed
/// reconciliation is all-or-nothing per list (see
/// `docs/CODE_STANDARDS.md`'s Interaction Semantics), and the root bubble/
/// divider above it are deliberately unkeyed (positional; there's only ever
/// one of each).
fn thread_content(
    controller: &Arc<MessagesController>,
    root: &FeedMessage,
    sheet_sig: RwSignal<ThreadSheet>,
) -> AnyView<HuddleState> {
    let mut children: Vec<forgekit::FlexChild<HuddleState>> =
        vec![inflexible(root_bubble(controller, root, sheet_sig))];

    if root.replies.is_empty() {
        children.push(inflexible(empty_replies_state()));
    } else {
        children.push(inflexible(divider_row(root.reply_count())));
        children.push(inflexible(reply_list(&root.replies, sheet_sig)));
    }

    let column = FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch);
    any(scroll_view(Padding(EdgeInsets::all(12.0), column)))
}

/// The keyed reply list — every child keyed by its index (replies are only
/// ever appended, never reordered/removed, so a stable index is a stable
/// identity here).
fn reply_list(replies: &[FeedReply], sheet_sig: RwSignal<ThreadSheet>) -> AnyView<HuddleState> {
    let children: Vec<forgekit::FlexChild<HuddleState>> = replies
        .iter()
        .enumerate()
        .map(|(idx, reply)| keyed(idx, reply_bubble(reply, sheet_sig)))
        .collect();
    any(FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch))
}

/// The root message, rendered prominently: avatar + author + time header,
/// body, and a reactions row — the same visual vocabulary as
/// `channel_feed::message_bubble`, just a single elevated card rather than a
/// left/right-aligned row (this is the thread's one parent, not a feed row).
fn root_bubble(
    controller: &Arc<MessagesController>,
    root: &FeedMessage,
    sheet_sig: RwSignal<ThreadSheet>,
) -> AnyView<HuddleState> {
    let author = mock::user(root.author_id)
        .map(|u| u.name)
        .unwrap_or("Someone");

    let header = any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(avatar(root.author_id)),
            inflexible(any(SizedBox(Some(8.0), None))),
            inflexible(any(
                text(format!("{author}  \u{b7}  {PLACEHOLDER_TIME}")).size(13.0)
            )),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center));

    let mut lines: Vec<AnyView<HuddleState>> = vec![
        header,
        any(SizedBox(None, Some(8.0))),
        root_body(&root.body),
    ];

    if let Some(reactions) = reaction_chips(controller, root) {
        lines.push(any(SizedBox(None, Some(6.0))));
        lines.push(reactions);
    }

    let inner = Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            lines.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Start),
    );

    // Long-press opens the root message's context menu (React/Copy/Delete).
    let id = root.id;
    any(
        GestureDetector(elevated_card(inner)).on_long_press(move |_st: &mut HuddleState| {
            sheet_sig.set(ThreadSheet::Menu(Some(id)));
        }),
    )
}

/// The root message's body — text, link preview, or file stub (mirrors
/// `channel_feed::message_body`).
fn root_body(body: &FeedBody) -> AnyView<HuddleState> {
    match body {
        FeedBody::Text(t) => any(text(t.clone()).size(16.0)),
        FeedBody::Link { url, title } => any(outlined_card(Padding(
            EdgeInsets::all(8.0),
            FlexView::new(
                Axis::Vertical,
                vec![
                    inflexible(any(FlexView::new(
                        Axis::Horizontal,
                        vec![
                            inflexible(any(icon(icons::LINK).size(16.0))),
                            inflexible(any(SizedBox(Some(6.0), None))),
                            inflexible(any(text(title.clone()).size(14.0))),
                        ],
                    ))),
                    inflexible(any(text(url.clone()).size(12.0))),
                ],
            )
            .cross_axis(CrossAxisAlignment::Start),
        ))),
        FeedBody::File { name, size } => any(outlined_card(Padding(
            EdgeInsets::all(8.0),
            FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(any(icon(icons::DESCRIPTION).size(24.0))),
                    inflexible(any(SizedBox(Some(8.0), None))),
                    inflexible(any(FlexView::new(
                        Axis::Vertical,
                        vec![
                            inflexible(any(text(name.clone()).size(14.0))),
                            inflexible(any(text(size.clone()).size(12.0))),
                        ],
                    )
                    .cross_axis(CrossAxisAlignment::Start))),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        ))),
    }
}

/// The reactions row under the root bubble, or `None` if it has no reactions
/// (mirrors `channel_feed::reaction_chips`, minus the own-message "hide the
/// add chip" carve-out — the root here is never rendered as "own" the way a
/// feed row is).
fn reaction_chips(
    controller: &Arc<MessagesController>,
    root: &FeedMessage,
) -> Option<AnyView<HuddleState>> {
    let mut chips: Vec<forgekit::FlexChild<HuddleState>> = Vec::new();
    for reaction in &root.reactions {
        let label = format!("{} {}", reaction.emoji, reaction.count);
        let emoji = reaction.emoji.clone();
        let ctrl = Arc::clone(controller);
        let id = root.id;
        chips.push(inflexible(any(forgekit::filter_chip::<HuddleState, _>(
            label,
            reaction.mine,
            move |_st: &mut HuddleState, _on: bool| ctrl.toggle_reaction(id, &emoji),
        ))));
        chips.push(inflexible(any(SizedBox(Some(6.0), None))));
    }

    if chips.is_empty() {
        return None;
    }

    Some(any(
        FlexView::new(Axis::Horizontal, chips).cross_axis(CrossAxisAlignment::Center)
    ))
}

/// A small avatar carrying the author's initials — duplicated locally rather
/// than shared with `channel_feed`'s own copy (each Phase C screen owns its
/// own visual helpers, keeping the two files disjoint per
/// `src/README-phase-c.md`).
fn avatar(author_id: u32) -> AnyView<HuddleState> {
    let initials = mock::user(author_id).map(|u| u.initials).unwrap_or("?");
    any(forgekit::filled_card(Padding(
        EdgeInsets::all(8.0),
        text(initials.to_string()).size(12.0),
    )))
}

/// The "N replies" divider row above the reply list.
fn divider_row(count: usize) -> AnyView<HuddleState> {
    let label = format!("{count} {}", if count == 1 { "reply" } else { "replies" });
    any(Padding(
        EdgeInsets::symmetric(4.0, 12.0),
        text(label).size(13.0),
    ))
}

/// The empty-thread state: "No replies yet — start the thread".
fn empty_replies_state() -> AnyView<HuddleState> {
    any(Padding(
        EdgeInsets::all(24.0),
        Align(
            Alignment::CENTER,
            text("No replies yet \u{2014} start the thread").size(14.0),
        ),
    ))
}

/// One compact reply bubble: author + text, no reactions/thread affordance
/// (the spec's "compact bubbles" — this thread has no nested sub-threads).
fn reply_bubble(reply: &FeedReply, sheet_sig: RwSignal<ThreadSheet>) -> AnyView<HuddleState> {
    let author = mock::user(reply.author_id)
        .map(|u| u.name)
        .unwrap_or("Someone");
    let inner = Padding(
        EdgeInsets::symmetric(12.0, 8.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(text(author.to_string()).size(12.0))),
                inflexible(any(text(reply.text.clone()).size(14.0))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    );
    // Long-press opens a Copy/Delete menu (replies carry no reaction model, so
    // no React row — `Menu(None)`).
    any(GestureDetector(Padding(
        EdgeInsets::symmetric(0.0, 4.0),
        outlined_card(inner),
    ))
    .on_long_press(move |_st: &mut HuddleState| {
        sheet_sig.set(ThreadSheet::Menu(None));
    }))
}

/// The reply composer: a `.multiline(5)` field plus a send (`icons::SEND`)
/// action calling [`MessagesController::reply_in_thread`] — mirrors
/// `channel_feed::composer_bar`, minus the attach/emoji affordance chips (not
/// in this screen's spec).
fn composer_bar(
    controller: Arc<MessagesController>,
    root_id: u32,
    composer: RwSignal<String>,
) -> AnyView<HuddleState> {
    let value = composer.get(); // tracked: drives the send button's enabled look
    let has_text = !value.trim().is_empty();

    let submit_ctrl = Arc::clone(&controller);
    let field = text_input(value, move |_s: &mut HuddleState, next: String| {
        composer.set(next);
    })
    .multiline(5)
    .placeholder("Reply")
    .on_submit(move |_s: &mut HuddleState, submitted: String| {
        submit_reply(&submit_ctrl, root_id, composer, submitted);
    });

    let send_icon = icon(icons::SEND).size(24.0);
    let send_btn: AnyView<HuddleState> = if has_text {
        any(
            GestureDetector(send_icon).on_tap(move |_s: &mut HuddleState| {
                let text = composer.get_untracked();
                submit_reply(&controller, root_id, composer, text);
            }),
        )
    } else {
        any(send_icon)
    };

    any(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                forgekit::flexible(1, any(field)),
                inflexible(any(SizedBox(Some(8.0), None))),
                inflexible(any(Padding(EdgeInsets::symmetric(0.0, 6.0), send_btn))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ))
}

/// Appends `text` to the thread (a no-op on empty/blank input) and clears the
/// composer.
fn submit_reply(
    controller: &Arc<MessagesController>,
    root_id: u32,
    composer: RwSignal<String>,
    text: String,
) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    controller.reply_in_thread(root_id, text);
    composer.set(String::new());
}
