//! Thread view (`/thread/:id`).
//!
//! The root message's full thread: the parent message rendered prominently
//! (header + body + reactions row), a "N replies" divider, the replies
//! (compact rows, keyed list), and a reply composer at the bottom —
//! consuming [`features::messages`](crate::features::messages)' controller
//! API (`message`/`replies_for`/`reply_count`/`reply_in_thread`, built for
//! this screen in task 12 — see that module's docs).
//!
//! # Flat rows (device-parity-round2 task R2b)
//!
//! The root/reply rows are now FLAT, matching `screens::channel_feed`'s own
//! R2 restyle: no `elevated_card`/`outlined_card` wrapper (this screen's own
//! last shadow-in-scroll site, per R2's B3 shadow check) — a plain
//! `Padding(12h/6v)` content column instead, and the avatar is a 40px
//! [`crate::ui::fill_box::fill_box`] disc rather than a `filled_card` tile.
//! Attachment tiles use [`crate::ui::fill_box::filled_box`] (`filled_card`
//! minus its 16px inset) exactly like `channel_feed::message_body`.
//!
//! # Why a thread-local composer, not a `Component` (mirrors `search`)
//!
//! `src/routes.rs`'s `/thread/:id` entry (a frozen hub file, see
//! `src/README-phase-c.md`) calls [`thread_screen`] with only the route
//! param, no `NavigatorController` — unlike `/channel/:id`, which gets one
//! (see `src/routes.rs`) — so, exactly like
//! [`search::presentation::pages::search`](crate::features::search::presentation::pages::search)'s
//! own documented reasoning,
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

use frust::{
    Align, Alignment, AnyView, Axis, Color, CrossAxisAlignment, EdgeInsets, FlexView,
    GestureDetector, Get, GetUntracked, Padding, RwSignal, Set, SizedBox, Stack, any, app_bar,
    hero, icon, icons, inflexible, keyed, scroll_view, text, text_input, use_context,
};
use kurbo::Size;

use crate::HuddleState;
use crate::features::messages::domain::repositories::MessageRepository;
use crate::features::messages::{FeedBody, FeedMessage, FeedReply, MessagesController};
use crate::ui::fill_box::{fill_box, filled_box};
use crate::ui::scaffold::placeholder_body;
use crate::ui::sheet::{action_menu, emoji_grid, sheet, sheet_action_row};
use crate::ui::swipeable::press_pop;

// ---------------------------------------------------------------------------
// Flat-row metrics (device-parity-round2 task R2b) — duplicated with the same
// names from `screens::channel_feed`'s own R2 constants rather than shared
// through a new module (R2b's spec: "prefer whatever is cleanest without
// creating a new module"); each screen owns its own visual helpers per
// `src/README-phase-c.md`'s Phase C disjointness convention (see this
// module's own `avatar` doc below).
// ---------------------------------------------------------------------------

/// Avatar tile side length — see `channel_feed::AVATAR_SIZE`.
const AVATAR_SIZE: f64 = 40.0;
/// Avatar corner radius: a full circle at [`AVATAR_SIZE`].
const AVATAR_RADIUS: f64 = AVATAR_SIZE / 2.0;
/// The initials monogram inside an avatar circle — see
/// `channel_feed::AVATAR_MONOGRAM_SIZE`.
const AVATAR_MONOGRAM_SIZE: f32 = 15.0;

/// Attachment-tile fill — see `channel_feed::TILE_FILL` (the M3
/// `surface_container_highest` value `filled_card` resolved to as its
/// unthemed fallback).
const TILE_FILL: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);

/// The initials-avatar background palette, indexed by user id — see
/// `channel_feed::AVATAR_PALETTE` (kept identical so the same author reads
/// the same color across screens).
const AVATAR_PALETTE: [Color; 6] = [
    Color::from_rgb8(0x1E, 0x88, 0xE5),
    Color::from_rgb8(0x8E, 0x24, 0xAA),
    Color::from_rgb8(0x00, 0x89, 0x7B),
    Color::from_rgb8(0xF4, 0x51, 0x1E),
    Color::from_rgb8(0x39, 0x49, 0xAB),
    Color::from_rgb8(0x6D, 0x4C, 0x41),
];

/// Avatar background for a user id — see `channel_feed::avatar_color`.
fn avatar_color(user_id: u32) -> Color {
    AVATAR_PALETTE[(user_id as usize) % AVATAR_PALETTE.len()]
}

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
        // Self-heal (task 22 hardening): a signal disposed by a prior owner
        // (reused test thread) returns `None` from `try_get_untracked` and is
        // recreated rather than handed back to panic on the next get/set — the
        // same fix `screens::channel_feed`/`screens::workspace_drawer` apply.
        if let Some(sig) = cell.borrow().get(&root_id).copied()
            && sig.try_get_untracked().is_some()
        {
            return sig;
        }
        let sig = RwSignal::new(String::new());
        cell.borrow_mut().insert(root_id, sig);
        sig
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
        // Same disposed-signal self-heal as `composer_for` above (task 22).
        if let Some(sig) = cell.borrow().get(&root_id).copied()
            && sig.try_get_untracked().is_some()
        {
            return sig;
        }
        let sig = RwSignal::new(ThreadSheet::None);
        cell.borrow_mut().insert(root_id, sig);
        sig
    })
}

/// The thread view for `thread_id` — the route param, parsed as the root
/// message's id.
pub fn thread_screen(thread_id: String) -> AnyView<HuddleState> {
    let Some(root_id) = thread_id.parse::<u32>().ok() else {
        return missing_thread_screen(&thread_id);
    };
    let repo = use_context::<Arc<dyn MessageRepository + Send + Sync>>()
        .expect("MessageRepository provided by the composition root");
    let Some(root_msg) = repo
        .messages()
        .into_iter()
        .chain(repo.firehose_messages())
        .find(|m| m.id == root_id)
    else {
        return missing_thread_screen(&thread_id);
    };

    let title = thread_title(&repo, root_msg.channel_id);
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
fn thread_title(repo: &Arc<dyn MessageRepository + Send + Sync>, channel_id: &str) -> String {
    if let Some(ch) = repo.channel(channel_id) {
        format!("Thread  \u{b7}  #{}", ch.name)
    } else if let Some(peer) = repo
        .dms()
        .iter()
        .find(|d| d.id == channel_id)
        .and_then(|d| repo.user(d.user_id))
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
    let mut children: Vec<frust::FlexChild<HuddleState>> = vec![
        inflexible(thread_app_bar(title)),
        frust::flexible(1, content),
    ];
    if let Some(composer) = composer {
        children.push(inflexible(composer));
    }
    any(FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch))
}

/// The fallback for an unresolvable route param (not a number, or no such
/// message id in the mock dataset) — mirrors
/// `profile::presentation::pages::profile`'s `unknown_user_screen` fallback.
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
    let mut children: Vec<frust::FlexChild<HuddleState>> =
        vec![inflexible(root_bubble(controller, root, sheet_sig))];

    if root.replies.is_empty() {
        children.push(inflexible(empty_replies_state()));
    } else {
        children.push(inflexible(divider_row(root.reply_count())));
        children.push(inflexible(reply_list(controller, &root.replies, sheet_sig)));
    }

    let column = FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch);
    any(scroll_view(Padding(EdgeInsets::all(12.0), column)))
}

/// The keyed reply list — every child keyed by its index (replies are only
/// ever appended, never reordered/removed, so a stable index is a stable
/// identity here).
fn reply_list(
    controller: &Arc<MessagesController>,
    replies: &[FeedReply],
    sheet_sig: RwSignal<ThreadSheet>,
) -> AnyView<HuddleState> {
    let children: Vec<frust::FlexChild<HuddleState>> = replies
        .iter()
        .enumerate()
        .map(|(idx, reply)| keyed(idx, reply_bubble(controller, reply, sheet_sig)))
        .collect();
    any(FlexView::new(Axis::Vertical, children).cross_axis(CrossAxisAlignment::Stretch))
}

/// The root message, rendered prominently: avatar + author + time header,
/// body, and a reactions row — the same FLAT visual vocabulary as
/// `channel_feed::message_bubble` (task R2b), just a single content column
/// rather than a left/right-aligned row (this is the thread's one parent, not
/// a feed row). No `elevated_card` wrapper any more — that was this screen's
/// last shadow-in-scroll site R2's B3 check flagged.
fn root_bubble(
    controller: &Arc<MessagesController>,
    root: &FeedMessage,
    sheet_sig: RwSignal<ThreadSheet>,
) -> AnyView<HuddleState> {
    let author = controller
        .user(root.author_id)
        .map(|u| u.name)
        .unwrap_or("Someone");

    let header = any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(avatar(controller, root.author_id)),
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

    // FLAT ROW (device-parity-round2 task R2b, mirroring
    // `channel_feed::message_bubble`): a plain padded content column
    // (12h/6v), no `elevated_card` wrapper.
    let inner = Padding(
        EdgeInsets::symmetric(12.0, 6.0),
        FlexView::new(
            Axis::Vertical,
            lines.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Start),
    );

    // Long-press opens the root message's context menu (React/Copy/Delete).
    let id = root.id;
    any(
        GestureDetector(inner).on_long_press(move |_st: &mut HuddleState| {
            sheet_sig.set(ThreadSheet::Menu(Some(id)));
        }),
    )
}

/// The root message's body — text, link preview, or file stub (mirrors
/// `channel_feed::message_body`). Attachment tiles keep a boxed look but
/// COMPACT (task R2b, matching R2's item 5 exactly): `filled_box(radius 8) +
/// Padding(10)`, not an `outlined_card` (whose 16px inset would balloon the
/// tile the same way it did the old feed — BUG.md B2).
fn root_body(body: &FeedBody) -> AnyView<HuddleState> {
    match body {
        FeedBody::Text(t) => any(text(t.clone()).size(16.0)),
        FeedBody::Link { url, title } => any(filled_box(
            Padding(
                EdgeInsets::all(10.0),
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
            ),
            TILE_FILL,
            8.0,
        )),
        FeedBody::File { name, size } => any(filled_box(
            Padding(
                EdgeInsets::all(10.0),
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
            ),
            TILE_FILL,
            8.0,
        )),
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
    let mut chips: Vec<frust::FlexChild<HuddleState>> = Vec::new();
    for reaction in &root.reactions {
        let label = format!("{} {}", reaction.emoji, reaction.count);
        let emoji = reaction.emoji.clone();
        let ctrl = Arc::clone(controller);
        let id = root.id;
        chips.push(inflexible(any(frust::filter_chip::<HuddleState, _>(
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

/// A 40px circular initials avatar — duplicated locally rather than shared
/// with `channel_feed`'s own copy (each Phase C screen owns its own visual
/// helpers, keeping the two files disjoint per `src/README-phase-c.md`), but
/// now built off the same `fill_box` disc + `SizedBox+Align` monogram idiom
/// as `channel_feed::avatar` (task R2b) rather than a `filled_card` tile.
/// Wrapped in a `hero("avatar-{author_id}")` shared element + a tap opening
/// the author's profile (`/user/:id`) — the same "avatar tap anywhere"
/// contract the feed avatar carries (task 22).
fn avatar(controller: &Arc<MessagesController>, author_id: u32) -> AnyView<HuddleState> {
    let initials = controller
        .user(author_id)
        .map(|u| u.initials)
        .unwrap_or("?");
    let tile = any(Stack(vec![
        any(fill_box(
            Size::new(AVATAR_SIZE, AVATAR_SIZE),
            avatar_color(author_id),
            AVATAR_RADIUS,
        )),
        any(SizedBox(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(Align(
            Alignment::CENTER,
            text(initials.to_string())
                .size(AVATAR_MONOGRAM_SIZE)
                .color(Color::WHITE),
        ))),
    ]));
    any(
        GestureDetector(hero(format!("avatar-{author_id}"), tile)).on_tap(
            move |st: &mut HuddleState| {
                st.nav.router().push(&format!("/user/{author_id}"));
            },
        ),
    )
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

/// One compact reply row: author + text, no reactions/thread affordance (the
/// spec's "compact bubbles" — this thread has no nested sub-threads). FLAT
/// (task R2b, matching `channel_feed::message_bubble`'s 12h/6v inset exactly)
/// — no `outlined_card` wrapper; the row carries no rounded chrome of its own
/// any more (a headless test locates a new reply by its glyph runs, not a
/// rounded-rect count — see `tests/thread.rs`).
fn reply_bubble(
    controller: &Arc<MessagesController>,
    reply: &FeedReply,
    sheet_sig: RwSignal<ThreadSheet>,
) -> AnyView<HuddleState> {
    let author = controller
        .user(reply.author_id)
        .map(|u| u.name)
        .unwrap_or("Someone");
    let inner = Padding(
        EdgeInsets::symmetric(12.0, 6.0),
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
    any(
        GestureDetector(Padding(EdgeInsets::symmetric(0.0, 4.0), inner)).on_long_press(
            move |_st: &mut HuddleState| {
                sheet_sig.set(ThreadSheet::Menu(None));
            },
        ),
    )
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
        // press_pop adds the pressed-state scale dip (task 22 micro-interaction).
        any(press_pop(GestureDetector(send_icon).on_tap(
            move |_s: &mut HuddleState| {
                let text = composer.get_untracked();
                submit_reply(&controller, root_id, composer, text);
            },
        )))
    } else {
        any(send_icon)
    };

    any(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                frust::flexible(1, any(field)),
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
