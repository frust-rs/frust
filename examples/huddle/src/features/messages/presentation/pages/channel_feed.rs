//! Channel / DM message feed (`/channel/:id`).
//!
//! The feed screen: message bubbles (own messages right-aligned, others left
//! with an avatar + name), link-preview and file-stub cards, reaction chips,
//! a thread affordance, a typing indicator, loading skeletons, near-start
//! pagination on `#firehose`, and the expanding multiline composer. All state
//! lives in the [`MessagesController`](crate::features::messages) this
//! screen sources via
//! [`MessagesController::for_channel`](crate::features::messages::MessagesController::for_channel)
//! — the shared per-channel-id registry that lets a reply composed
//! in an open [`thread`](super::thread) update this same screen's rendered
//! reply count, and vice versa.
//!
//! # Why a plain function, not a `Component` (mirrors `screens::thread`)
//!
//! An earlier version of this screen hosted its controller behind a
//! `Component`'s retained state (via `clean_signals_frust::use_controller`).
//! That fights the shared registry: a `Component`'s local state lives behind
//! its own nested reactive `Owner`
//! (`docs/ARCHITECTURE.md`'s Component state boundary), disposed when the
//! page is torn down — fine for a controller genuinely owned by one screen,
//! but not for one meant to outlive any single mount and be handed to a
//! *different* screen (`screens::thread`) too. `channel_feed` is now a
//! **plain** function, exactly the `screens::thread`/`features::search`/
//! `features::activity` precedent: the shared controller comes from
//! [`MessagesController::for_channel`], and only the composer's live text is
//! genuinely screen-local, cached across rebuilds behind a `thread_local!`
//! map keyed by channel id (this screen's counterpart to `screens::thread`'s
//! `composer_for`, keyed there by root message id instead).
//!
//! # Feed container: a keyed, variable-extent `ListView`
//!
//! The feed renders through
//! [`ListView::builder_keyed`](frust::ListView::builder_keyed) over one flat
//! row list — the "loading older…" and "being typed…" singleton rows (when
//! shown) bracketing a [`FeedRow::Message`] per loaded message, rebuilt fresh
//! from
//! [`MessagesController::feed`](crate::features::messages::MessagesController::feed)
//! every frame exactly like the earlier `keyed` `Column`'s children were —
//! keyed by [`FeedRow::key`] (message id for a real message, the same
//! collision-proof string keys the earlier list used for the two
//! singletons). [`FEED_ROW_ESTIMATE`] feeds
//! [`estimated_item_extent`](frust::ListView::estimated_item_extent),
//! switching the list into variable-extent mode: every row still measures and
//! lays out at its own real height (an "other" row's avatar, a link/file
//! card, reaction chips, the typing row's spinner) — the estimate is only
//! ever the *assumed* height of a row the list hasn't laid out yet.
//! [`on_near_start`](frust::ListView::on_near_start) at [`NEAR_START_PX`]
//! drives the "load older" pagination trigger, and the list's own **prepend
//! anchoring** (see `frust_widgets::list_view`'s module docs) is what keeps
//! the viewport visually stationary the instant
//! [`load_older`](crate::features::messages::MessagesController::load_older)
//! prepends a page — this screen carries no scroll-position bookkeeping of
//! its own.
//!
//! An earlier version of this screen could not reach for `ListView` at all:
//! at the time it required a single uniform `item_extent` and reconciled rows
//! by raw index, neither compatible with variable-height chat bubbles or the
//! keyed identity a mid-list-safe feed needs, so the feed was a
//! [`scroll_view`](frust::scroll_view) over a [`keyed`](frust::keyed)
//! [`Column`](frust::Column) instead, with the near-start trigger emulated
//! off [`ScrollView::on_scroll`](frust::ScrollView). `ListView::builder_keyed`
//! plus `estimated_item_extent` (`frust-widgets`' own later addition) close
//! exactly that gap, so the feed now uses the container the original design
//! named.
//!
//! # One remaining adaptation to the facade-only widget set
//!
//! This example crate depends on the `frust` facade **alone** (see
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions), so one visual
//! item is still expressed with the widgets the facade ships rather than a
//! bespoke `Widget`:
//!
//! - **Per-message entrance + typing indicator.** A per-item slide/fade
//!   `AnimationController` would need a custom painting `Widget`, authored via
//!   `frust::authoring` — the seam `ui::toast`'s own entrance wrapper uses —
//!   but this screen stays on the facade's built-in widgets alone rather than
//!   hand-rolling one here. The keyed list gives newly-appended messages
//!   stable identity, and the "being typed" affordance uses the facade's
//!   self-animating
//!   [`loading_indicator`](frust::loading_indicator) /
//!   [`cupertino_activity_indicator`](frust::cupertino_activity_indicator)
//!   as a live animation. Message rows are FLAT (no `Card` wrapper) — the
//!   `filled_card`/`elevated_card` bubble
//!   backgrounds were removed because the card's hardcoded 16px inset inflated
//!   the whole feed; own-vs-others is carried by row layout (own
//!   right-aligned, others left with an avatar), not a background tint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use frust::{
    Align, Alignment, AnyView, Axis, ChildKey, Color, CrossAxisAlignment, DesignLanguage,
    EdgeInsets, FlexView, GestureDetector, Get, GetUntracked, ListView, NavigatorController,
    Padding, RwSignal, Set, SizedBox, Stack, Theme, Update, any, app_bar, assist_chip,
    cupertino_activity_indicator, filter_chip, flexible, hero, icon, icons, inflexible,
    kurbo::Size, loading_indicator, safe_area, text, text_input, use_context,
};

use crate::HuddleState;
use crate::features::messages::{FeedBody, FeedMessage, MessagesController};
use crate::ui::fill_box::{fill_box, filled_box};
use crate::ui::sheet::{action_menu, avoid_keyboard, emoji_grid, sheet, sheet_action_row};
use crate::ui::swipeable::{SwipeMarker, press_pop, swipeable_row};

/// Distance (logical px) from the top of the feed at which the near-start
/// "load older" trigger fires — the infinite-scroll edge for `#firehose`.
const NEAR_START_PX: f64 = 96.0;

/// [`ListView::estimated_item_extent`](frust::ListView::estimated_item_extent)'s
/// assumed height for a row the list hasn't laid out yet — a single-line
/// "other" message row (sender line + one body line, the more chrome-heavy of
/// the two row shapes, since it also carries the avatar column), picked from
/// `Theme`'s M3 type scale rather than guessed: [`message_row`]'s own 4px
/// top+bottom row padding (8) + [`message_bubble`]'s own 6px top+bottom
/// bubble padding (12) + a meta line sized off `TypeScale::label_large`'s
/// 20px line-height (nearest scale role to the row's literal 13px
/// sender/timestamp text) + a body line sized off `TypeScale::body_large`'s
/// 24px line-height (nearest scale role to the row's literal 15px body
/// text) = 8 + 12 + 20 + 24 = 64. Only ever the *assumed* height — a
/// materialized row always measures and lays out its own real height (see
/// the module docs' *Feed container* section).
const FEED_ROW_ESTIMATE: f64 = 64.0;

/// Swipe-to-reply strip accent — a Slack-familiar blue.
const REPLY_COLOR: Color = Color::from_rgb8(0x1E, 0x88, 0xE5);

// ---------------------------------------------------------------------------
// Flat-row metrics — the numeric
// contract that de-cards the feed off `filled_card`'s 16px inset driver.
// ---------------------------------------------------------------------------

/// Chat-row avatar tile side length (36–40 per the chat-row sizing table);
/// a full circle at [`AVATAR_RADIUS`]. Replaces the old ~62px `filled_card`
/// avatar (40 total).
const AVATAR_SIZE: f64 = 40.0;
/// Avatar corner radius: a full circle at [`AVATAR_SIZE`].
const AVATAR_RADIUS: f64 = AVATAR_SIZE / 2.0;
/// The initials monogram inside an avatar circle (a bodyLarge-ish glyph).
const AVATAR_MONOGRAM_SIZE: f32 = 15.0;

/// Composer attach/emoji tap-tile side length (24 glyph in a 44 tile — the
/// Material min-touch target). Replaces the old ~72px `filled_card` button
/// (44 total, 24 glyph).
const COMPOSER_TILE: f64 = 44.0;
/// Composer tile corner radius — a rounded square, not the avatar's full circle.
const COMPOSER_TILE_RADIUS: f64 = 12.0;

/// Composer tile / attachment-card fill — the M3 `surface_container_highest`
/// value `filled_card` resolved to as its unthemed fallback, reused here as a
/// bare constant since the facade exposes no color-scheme role to app code.
const TILE_FILL: Color = Color::from_rgb8(0xE6, 0xE0, 0xE9);

/// The initials-avatar background palette, indexed by user id (mirrors
/// `screens::home`'s palette so the same author reads the same color).
const AVATAR_PALETTE: [Color; 6] = [
    Color::from_rgb8(0x1E, 0x88, 0xE5),
    Color::from_rgb8(0x8E, 0x24, 0xAA),
    Color::from_rgb8(0x00, 0x89, 0x7B),
    Color::from_rgb8(0xF4, 0x51, 0x1E),
    Color::from_rgb8(0x39, 0x49, 0xAB),
    Color::from_rgb8(0x6D, 0x4C, 0x41),
];

/// Avatar background for a user id.
fn avatar_color(user_id: u32) -> Color {
    AVATAR_PALETTE[(user_id as usize) % AVATAR_PALETTE.len()]
}

/// Skeleton placeholder-bar fill (a neutral grey).
const SKELETON_FILL: Color = Color::from_rgb8(0xE0, 0xE0, 0xE0);
/// Skeleton placeholder-bar height; wrapped in 6px vertical padding it makes a
/// ~52px flat row (down from the old 92px card row).
const SKELETON_HEIGHT: f64 = 40.0;
/// How many skeleton bars the loading state shows.
const SKELETON_COUNT: usize = 6;

/// A quick-react emoji offered under every bubble (the "add reaction"
/// affordance). Real color-glyph rendering, already verified to render.
const QUICK_REACT: &str = "\u{1F44D}";

/// Gets (or creates) the thread-local composer text for `channel_id` — see
/// the module docs' "Why a plain function" section. The controller itself is
/// shared (see
/// [`MessagesController::for_channel`](crate::features::messages::MessagesController::for_channel));
/// only the in-flight composer text is screen-local, keyed here so a
/// different channel never bleeds into this one's composer.
fn composer_for(channel_id: &str) -> RwSignal<String> {
    thread_local! {
        static COMPOSERS: RefCell<HashMap<String, RwSignal<String>>> =
            RefCell::new(HashMap::new());
    }
    COMPOSERS.with(|cell| {
        // Self-heal: the cached signal is owned by whatever
        // reactive `Owner` was live when it was created; a headless test that
        // disposes its owner and re-enters (or a reused test thread) leaves a
        // *disposed* signal here whose next get/set panics. `try_get_untracked`
        // returning `None` detects that, so we recreate rather than hand it back.
        if let Some(sig) = cell.borrow().get(channel_id).copied()
            && sig.try_get_untracked().is_some()
        {
            return sig;
        }
        let sig = RwSignal::new(String::new());
        cell.borrow_mut().insert(channel_id.to_string(), sig);
        sig
    })
}

/// Which message-action sheet (if any) is open over the feed — the
/// long-press menu / emoji picker / attachment sheet. Screen-local
/// state, cached across rebuilds behind [`sheet_for`] exactly like the composer
/// text, since this screen is a plain function with no `Component` state.
#[derive(Clone, PartialEq)]
enum FeedSheet {
    /// No sheet — the feed is interactive.
    None,
    /// The long-press context menu for message `id`.
    Menu(u32),
    /// The emoji reaction picker, targeting a message or the composer.
    Emoji(EmojiTarget),
    /// The mock attachment sheet (Photo / File / Poll).
    Attach,
}

/// Where a picked emoji goes: onto a message's reactions, or inserted into the
/// composer text.
#[derive(Clone, Copy, PartialEq)]
enum EmojiTarget {
    /// Toggle the reaction on message `id`.
    Message(u32),
    /// Append the emoji to the composer field.
    Composer,
}

/// Gets (or creates) the screen-local open-sheet signal for `channel_id` — the
/// counterpart to [`composer_for`], keyed the same way so two channels never
/// share one sheet.
fn sheet_for(channel_id: &str) -> RwSignal<FeedSheet> {
    thread_local! {
        static SHEETS: RefCell<HashMap<String, RwSignal<FeedSheet>>> =
            RefCell::new(HashMap::new());
    }
    SHEETS.with(|cell| {
        // Same disposed-signal self-heal as `composer_for` above.
        if let Some(sig) = cell.borrow().get(channel_id).copied()
            && sig.try_get_untracked().is_some()
        {
            return sig;
        }
        let sig = RwSignal::new(FeedSheet::None);
        cell.borrow_mut().insert(channel_id.to_string(), sig);
        sig
    })
}

/// The channel/DM feed route entry point. `navigator` lets the screen pop back
/// and push into a thread; `channel_id` selects the conversation.
pub fn channel_feed(
    navigator: NavigatorController<HuddleState>,
    channel_id: String,
) -> AnyView<HuddleState> {
    let controller = MessagesController::for_channel(channel_id.clone());
    let composer = composer_for(&channel_id);
    let sheet_sig = sheet_for(&channel_id);
    let design = use_context::<Theme>()
        .map(|t| t.design_language)
        .unwrap_or(DesignLanguage::Material3);

    let bar = feed_app_bar(&controller, &channel_id, navigator);
    let body = feed_body(&controller, design, sheet_sig);
    let composer_row = composer_bar(&controller, composer, sheet_sig);

    let screen = any(FlexView::new(
        Axis::Vertical,
        vec![inflexible(bar), flexible(1, body), inflexible(composer_row)],
    )
    .cross_axis(CrossAxisAlignment::Stretch));

    // The message-action sheet mounts in the screen's own `Stack` top layer
    // (see `crate::ui::sheet`); when closed it is an inert zero-size box, so the
    // feed stays interactive.
    let overlay = feed_sheet(&controller, composer, sheet_sig);
    any(Stack(vec![screen, overlay]))
}

/// The open message-action sheet as a `Stack` overlay layer, or an inert
/// zero-size box when nothing is open. Reads the tracked [`FeedSheet`] signal so
/// a menu/picker open (or a dismiss) wakes this rebuild.
fn feed_sheet(
    controller: &Arc<MessagesController>,
    composer: RwSignal<String>,
    sheet_sig: RwSignal<FeedSheet>,
) -> AnyView<HuddleState> {
    let dismiss = move |_st: &mut HuddleState| sheet_sig.set(FeedSheet::None);
    match sheet_sig.get() {
        FeedSheet::None => any(SizedBox(None, None)),
        FeedSheet::Menu(id) => {
            any(sheet(action_menu(feed_menu_rows(id, sheet_sig))).on_dismiss(dismiss))
        }
        FeedSheet::Emoji(target) => {
            let ctrl = Arc::clone(controller);
            let picker = emoji_grid(move |_st: &mut HuddleState, emoji: &'static str| {
                match target {
                    EmojiTarget::Message(id) => ctrl.toggle_reaction(id, emoji),
                    EmojiTarget::Composer => composer.update(|s| s.push_str(emoji)),
                }
                sheet_sig.set(FeedSheet::None);
            });
            any(sheet(picker).on_dismiss(dismiss))
        }
        FeedSheet::Attach => {
            any(sheet(action_menu(attachment_rows(sheet_sig))).on_dismiss(dismiss))
        }
    }
}

/// The long-press context-menu rows for message `id`: React (opens the picker),
/// Reply in thread (pushes `/thread/:id`), Copy (mock toast), Delete (mock
/// toast). Each closes the menu.
fn feed_menu_rows(id: u32, sheet_sig: RwSignal<FeedSheet>) -> Vec<AnyView<HuddleState>> {
    vec![
        sheet_action_row(icons::MOOD, "React", move |_st: &mut HuddleState| {
            sheet_sig.set(FeedSheet::Emoji(EmojiTarget::Message(id)));
        }),
        sheet_action_row(
            icons::REPLY,
            "Reply in thread",
            move |st: &mut HuddleState| {
                sheet_sig.set(FeedSheet::None);
                st.nav.router().push(&format!("/thread/{id}"));
            },
        ),
        sheet_action_row(icons::DESCRIPTION, "Copy", move |st: &mut HuddleState| {
            sheet_sig.set(FeedSheet::None);
            st.toasts.show("Copied");
        }),
        sheet_action_row(icons::DELETE, "Delete", move |st: &mut HuddleState| {
            sheet_sig.set(FeedSheet::None);
            st.toasts.show("Only admins can delete \u{2014} mock");
        }),
    ]
}

/// The mock attachment-sheet rows: Photo / File / Poll, each toasting and
/// closing the sheet.
fn attachment_rows(sheet_sig: RwSignal<FeedSheet>) -> Vec<AnyView<HuddleState>> {
    let row = |leading, label: &'static str, kind: &'static str| {
        sheet_action_row(leading, label, move |st: &mut HuddleState| {
            sheet_sig.set(FeedSheet::None);
            st.toasts.show(format!("Attached {kind} (mock)"));
        })
    };
    vec![
        row(icons::IMAGE, "Photo", "Photo"),
        row(icons::ATTACH_FILE, "File", "File"),
        row(icons::FORUM, "Poll", "Poll"),
    ]
}

/// The app bar: a back button, the channel/DM title, and a member-count
/// subtitle folded into the title (the M3 app bar has one title slot).
fn feed_app_bar(
    controller: &Arc<MessagesController>,
    channel_id: &str,
    navigator: NavigatorController<HuddleState>,
) -> AnyView<HuddleState> {
    let title = if let Some(ch) = controller.channel(channel_id) {
        let members = controller.users().len();
        format!("#{}  ·  {members} members", ch.name)
    } else if let Some(peer) = controller
        .dms()
        .iter()
        .find(|d| d.id == channel_id)
        .and_then(|d| controller.user(d.user_id))
    {
        format!("{}  ·  {}", peer.name, peer.status.label())
    } else {
        channel_id.to_string()
    };

    let back = frust::GestureDetector(icon(icons::ARROW_BACK).size(24.0))
        .on_tap(move |_st: &mut HuddleState| navigator.pop());

    // Clears the top status-bar/cutout inset — see `shell::bottom_bar`'s doc
    // for the matching known v1 background-extension gap.
    any(safe_area(
        app_bar::<HuddleState>(title).leading(any(Padding(EdgeInsets::symmetric(4.0, 0.0), back))),
    )
    .bottom(false))
}

/// One row of the virtualized feed list — the two ephemeral singleton rows
/// bracketing a real message, exactly the shape the earlier `keyed` `Column`
/// built by hand each frame. See the [module docs](self)' *Feed container*
/// section.
enum FeedRow {
    /// The "loading older…" row, shown at the top while an older page loads.
    LoadingOlder,
    /// A real message.
    Message(FeedMessage),
    /// The "being typed…" row, shown at the bottom while a reply is pending.
    Typing,
}

impl FeedRow {
    /// This row's stable [`ChildKey`] — a message's id for
    /// [`FeedRow::Message`], the same collision-proof string keys the earlier
    /// list used for its two singletons otherwise (distinct from any message
    /// id: [`ChildKey::new`] hashes the value *and* its type, and a message
    /// id is a `u32`, never a `&str`).
    fn key(&self) -> ChildKey {
        match self {
            FeedRow::LoadingOlder => ChildKey::new("loading_older"),
            FeedRow::Message(msg) => ChildKey::new(msg.id),
            FeedRow::Typing => ChildKey::new("typing"),
        }
    }
}

/// The scrolling feed area: skeletons while loading, else the keyed,
/// variable-extent `ListView` with the near-start pagination trigger. See the
/// [module docs](self)' *Feed container* section.
fn feed_body(
    controller: &Arc<MessagesController>,
    design: DesignLanguage,
    sheet_sig: RwSignal<FeedSheet>,
) -> AnyView<HuddleState> {
    if controller.loading.get() {
        return skeletons();
    }

    let messages = controller.feed(); // tracked
    let loading_older = controller.loading_older.get();
    let typing = controller.typing.get();

    // Empty conversation (e.g. a DM with no messages): a real empty state
    // rather than a blank scroll area.
    if messages.is_empty() && !loading_older && !typing {
        return empty_feed_state();
    }

    let mut rows: Vec<FeedRow> = Vec::with_capacity(messages.len() + 2);
    if loading_older {
        rows.push(FeedRow::LoadingOlder);
    }
    rows.extend(messages.into_iter().map(FeedRow::Message));
    if typing {
        rows.push(FeedRow::Typing);
    }
    let row_count = rows.len();
    let rows = Rc::new(rows);

    let key_rows = Rc::clone(&rows);
    let key_of = move |i: usize| key_rows[i].key();

    let build_rows = Rc::clone(&rows);
    let build_controller = Arc::clone(controller);
    let builder =
        move |i: usize| feed_row_view(&build_controller, &build_rows[i], sheet_sig, design);

    let pager = Arc::clone(controller);
    let list = ListView::builder_keyed(row_count, FEED_ROW_ESTIMATE, key_of, builder)
        .estimated_item_extent(FEED_ROW_ESTIMATE)
        .on_near_start(
            move |_st: &mut HuddleState| {
                if pager.has_more.get_untracked() && !pager.loading_older.get_untracked() {
                    let pager = Arc::clone(&pager);
                    frust::spawn(async move {
                        pager.load_older().await;
                    });
                }
            },
            NEAR_START_PX,
        );

    // Flat-row outer HORIZONTAL padding is 8px (the message rows themselves
    // add no card inset any more); the old scroll-view's 12px vertical
    // breathing room around the whole scrollable column is folded onto the
    // list's own viewport here instead of the (now virtualized) content, so
    // it no longer scrolls away with the first/last row — a cosmetic-only
    // difference from the pre-`ListView` feed.
    any(Padding(EdgeInsets::symmetric(8.0, 12.0), list))
}

/// One feed row's view: the two singleton rows unchanged from the earlier
/// keyed `Column`, or a real message row.
fn feed_row_view(
    controller: &Arc<MessagesController>,
    row: &FeedRow,
    sheet_sig: RwSignal<FeedSheet>,
    design: DesignLanguage,
) -> AnyView<HuddleState> {
    match row {
        FeedRow::LoadingOlder => loading_older_row(design),
        FeedRow::Message(msg) => message_row(controller, msg, sheet_sig),
        FeedRow::Typing => typing_row(design),
    }
}

/// One message row: the bubble, aligned right (own) or left (others, with an
/// avatar), inside horizontal breathing room. An other-user's row is wrapped in
/// a swipe-to-reply [`swipeable_row`]: a swipe right reveals the reply
/// affordance and, on commit, opens the message's thread (`/thread/:id`) —
/// Slack-familiar. It composes with the bubble's own long-press + tap
/// exactly as the Home rows compose swipe + long-press (see
/// `screens::home`): the swipeable forwards every pointer event to its child
/// until a horizontal drag crosses the slop, so the bubble's long-press menu and
/// the avatar/chip taps inside it stay live.
fn message_row(
    controller: &Arc<MessagesController>,
    msg: &FeedMessage,
    sheet_sig: RwSignal<FeedSheet>,
) -> AnyView<HuddleState> {
    let bubble = message_bubble(controller, msg, sheet_sig);
    if msg.is_own() {
        let row = FlexView::new(
            Axis::Horizontal,
            vec![flexible(1, any(SizedBox(None, None))), inflexible(bubble)],
        );
        return any(Padding(EdgeInsets::symmetric(0.0, 4.0), row));
    }

    let row = FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(avatar(controller, msg.author_id)),
            inflexible(any(SizedBox(Some(8.0), None))),
            inflexible(bubble),
            flexible(1, any(SizedBox(None, None))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Start);

    // Swipe right → open the thread (starting it if the message has none yet —
    // the thread screen already handles an empty thread).
    let root_id = msg.id;
    let reply = swipeable_row(row).on_swipe_right_marked(
        REPLY_COLOR,
        SwipeMarker::Reply,
        move |st: &mut HuddleState| {
            st.nav.router().push(&format!("/thread/{root_id}"));
        },
    );
    any(Padding(EdgeInsets::symmetric(0.0, 4.0), reply))
}

/// The flat message content: author line (others only), body, reaction chips,
/// and the thread affordance, in a plain padded column (no card).
/// Wrapped in a [`GestureDetector`] whose long-press opens the message's
/// context menu; the detector is transparent, so the reaction chips
/// and thread affordance inside still tap through.
fn message_bubble(
    controller: &Arc<MessagesController>,
    msg: &FeedMessage,
    sheet_sig: RwSignal<FeedSheet>,
) -> AnyView<HuddleState> {
    let mut lines: Vec<AnyView<HuddleState>> = Vec::new();

    if !msg.is_own()
        && let Some(user) = controller.user(msg.author_id)
    {
        // Sender + timestamp on one line: 13 (a combined meta line between the
        // sizing table's sender-15 and timestamp-12 roles — `Text` has no
        // mixed-run weight/size, so it's one size, per the chat-row conventions).
        lines.push(any(text(format!("{}  ·  9:41 AM", user.name)).size(13.0)));
    }

    lines.push(message_body(msg));

    let reactions = reaction_chips(controller, msg);
    if let Some(reactions) = reactions {
        lines.push(reactions);
    }

    if msg.reply_count() > 0 {
        lines.push(thread_affordance(msg));
    }

    // FLAT ROW: no `filled_card`/
    // `elevated_card` wrapper — that card's hardcoded 16px inset was the size
    // driver the previous restyle failed to escape. A message is now a plain
    // padded content column (12h / 6v), Slack-flat. Own-vs-others is carried by
    // the row layout (own right-aligned, others left with an avatar — see
    // `message_row`), NOT a per-bubble background tint (see RESULT: dropped as
    // not trivially expressible full-width behind variable-height content).
    let inner = Padding(
        EdgeInsets::symmetric(12.0, 6.0),
        FlexView::new(
            Axis::Vertical,
            lines.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Start),
    );

    let id = msg.id;
    any(
        GestureDetector(inner).on_long_press(move |_st: &mut HuddleState| {
            sheet_sig.set(FeedSheet::Menu(id));
        }),
    )
}

/// The message body: plain text, a link-preview card, or a file-stub card.
fn message_body(msg: &FeedMessage) -> AnyView<HuddleState> {
    match &msg.body {
        FeedBody::Text(t) => any(text(t.clone()).size(15.0)),
        // Attachment tiles keep a boxed look but COMPACT:
        // `filled_box(radius 8) + Padding(10)`, not a `filled_card`/
        // `outlined_card` (whose 16px inset would balloon the tile like it did
        // the whole feed).
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

/// The reaction chips row under a bubble (each toggles the current user's
/// reaction), plus a quick-react "add" chip. `None` if there is nothing to show
/// and the message is the current user's own (own messages skip the add chip to
/// keep the trailing edge clean).
fn reaction_chips(
    controller: &Arc<MessagesController>,
    msg: &FeedMessage,
) -> Option<AnyView<HuddleState>> {
    if msg.reactions.is_empty() && msg.is_own() {
        return None;
    }

    let mut chips: Vec<frust::FlexChild<HuddleState>> = Vec::new();
    for reaction in &msg.reactions {
        let label = format!("{} {}", reaction.emoji, reaction.count);
        let emoji = reaction.emoji.clone();
        let ctrl = Arc::clone(controller);
        let id = msg.id;
        chips.push(inflexible(any(filter_chip::<HuddleState, _>(
            label,
            reaction.mine,
            move |_st: &mut HuddleState, _on: bool| ctrl.toggle_reaction(id, &emoji),
        ))));
        chips.push(inflexible(any(SizedBox(Some(6.0), None))));
    }

    // The quick-react add chip.
    let ctrl = Arc::clone(controller);
    let id = msg.id;
    chips.push(inflexible(any(assist_chip::<HuddleState, _>(
        QUICK_REACT,
        move |_st: &mut HuddleState| ctrl.toggle_reaction(id, QUICK_REACT),
    )
    .leading("+"))));

    Some(any(Padding(
        EdgeInsets::symmetric(0.0, 6.0),
        FlexView::new(Axis::Horizontal, chips).cross_axis(CrossAxisAlignment::Center),
    )))
}

/// The "N replies →" thread affordance; tapping pushes `/thread/:id` via
/// `state.nav`'s router (mirroring `screens::thread`'s own `s.nav.router()`
/// back-action — this plain fn has no captured `NavigatorController` of its
/// own to push imperatively with).
fn thread_affordance(msg: &FeedMessage) -> AnyView<HuddleState> {
    let count = msg.reply_count();
    let label = format!(
        "{count} {} \u{2192}",
        if count == 1 { "reply" } else { "replies" }
    );
    let id = msg.id;
    any(Padding(
        EdgeInsets::symmetric(0.0, 4.0),
        assist_chip::<HuddleState, _>(label, move |st: &mut HuddleState| {
            st.nav.router().push(&format!("/thread/{id}"));
        })
        .leading("\u{1F4AC}"),
    ))
}

/// A 40px circular initials avatar (chat-row avatar 36–40, per the sizing
/// table),
/// built off the direct-sized [`fill_box`] disc + the `SizedBox+Align` monogram
/// idiom (`screens::home::channel_circle` / `profile.rs`) rather than a
/// `filled_card`, whose 16px inset ballooned the old tile to ~62px.
/// Wrapped in a `hero("avatar-{author_id}")` shared element + a tap that opens
/// the author's profile (`/user/:id`) — completing the "avatar tap anywhere"
/// matrix row alongside Home/Search/Activity.
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

/// The empty-conversation state: shown when a channel/DM feed has loaded with no
/// messages (a DM with no history) instead of a blank scroll area.
fn empty_feed_state() -> AnyView<HuddleState> {
    any(Padding(
        EdgeInsets::all(24.0),
        Align(
            Alignment::CENTER,
            text("No messages yet \u{2014} say hi \u{1F44B}").size(15.0),
        ),
    ))
}

/// The typing indicator row (a self-animating facade spinner + label).
fn typing_row(design: DesignLanguage) -> AnyView<HuddleState> {
    let spinner: AnyView<HuddleState> = match design {
        DesignLanguage::Cupertino => any(cupertino_activity_indicator()),
        // Glyph has no spinner chrome baseline yet —
        // falls through to the Material3 arm for now.
        DesignLanguage::Material3 | DesignLanguage::Glyph => any(loading_indicator()),
    };
    any(Padding(
        EdgeInsets::symmetric(4.0, 8.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(spinner),
                inflexible(any(SizedBox(Some(8.0), None))),
                inflexible(any(text("typing\u{2026}").size(13.0))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ))
}

/// The "loading older…" row shown at the top while an older page is fetched.
fn loading_older_row(design: DesignLanguage) -> AnyView<HuddleState> {
    let spinner: AnyView<HuddleState> = match design {
        DesignLanguage::Cupertino => any(cupertino_activity_indicator()),
        // Glyph has no spinner chrome baseline yet —
        // falls through to the Material3 arm for now.
        DesignLanguage::Material3 | DesignLanguage::Glyph => any(loading_indicator()),
    };
    any(Padding(
        EdgeInsets::all(8.0),
        frust::Align(
            frust::Alignment::CENTER,
            FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(spinner),
                    inflexible(any(SizedBox(Some(8.0), None))),
                    inflexible(any(text("loading older\u{2026}").size(13.0))),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        ),
    ))
}

/// The loading skeleton: a column of flat grey placeholder bars. Each bar is a
/// stretched [`fill_box`] (full row width, [`SKELETON_HEIGHT`] tall) in 6px
/// vertical padding — a ~52px flat row, down from the old 92px `filled_card`
/// row whose 16px inset was the size driver.
/// Under `CrossAxisAlignment::Stretch` the fill_box's nominal width
/// is overridden by the tight cross-axis constraint, so it fills the row.
fn skeletons() -> AnyView<HuddleState> {
    let mut rows: Vec<AnyView<HuddleState>> = Vec::new();
    for _ in 0..SKELETON_COUNT {
        rows.push(any(Padding(
            EdgeInsets::symmetric(0.0, 6.0),
            fill_box(Size::new(0.0, SKELETON_HEIGHT), SKELETON_FILL, 8.0),
        )));
    }
    any(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Vertical,
            rows.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

/// The composer: an affordance chip row (shown only when there is text), the
/// expanding multiline field, and the send button.
fn composer_bar(
    controller: &Arc<MessagesController>,
    composer: RwSignal<String>,
    sheet_sig: RwSignal<FeedSheet>,
) -> AnyView<HuddleState> {
    let value = composer.get(); // tracked
    let has_text = !value.trim().is_empty();

    // Always-visible attach + emoji affordance buttons (originally inert
    // placeholders, now wired). Each opens its own sheet.
    let attach_btn = affordance_button(icons::ATTACH_FILE, move |_st: &mut HuddleState| {
        sheet_sig.set(FeedSheet::Attach);
    });
    let emoji_btn = affordance_button(icons::MOOD, move |_st: &mut HuddleState| {
        sheet_sig.set(FeedSheet::Emoji(EmojiTarget::Composer));
    });

    // The multiline field: Enter inserts a newline, the send button / Shift+Enter
    // submits (the multiline default — see `TextInput::multiline`).
    let submit_ctrl = Arc::clone(controller);
    let field = text_input(value.clone(), move |_st: &mut HuddleState, next: String| {
        composer.set(next);
    })
    .multiline(5)
    .placeholder("Message")
    .on_submit(move |_st: &mut HuddleState, submitted: String| {
        submit(&submit_ctrl, composer, submitted);
    });

    let send_ctrl = Arc::clone(controller);
    let send_icon = icon(icons::SEND).size(24.0);
    let send_btn: AnyView<HuddleState> = if has_text {
        // press_pop adds the pressed-state scale dip.
        any(press_pop(frust::GestureDetector(send_icon).on_tap(
            move |_st: &mut HuddleState| {
                let text = composer.get_untracked();
                submit(&send_ctrl, composer, text);
            },
        )))
    } else {
        // Empty: the button is inert (no handler), just the glyph.
        any(send_icon)
    };

    // Rides above the on-screen keyboard: the
    // composer sits at the window bottom under edge-to-edge + `adjustResize`
    // (see `templates/app/android.tmpl` / `examples/huddle/android`'s
    // wiring), so it must consume the raw IME occlusion itself rather than
    // relying on a window resize. `avoid_keyboard` pads by the live
    // `WindowInsets::view_insets.bottom` — the same raw inset `ui::sheet`'s
    // bottom-anchored panel consumes for its own keyboard avoidance.
    any(avoid_keyboard(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(attach_btn),
                inflexible(any(SizedBox(Some(4.0), None))),
                inflexible(emoji_btn),
                inflexible(any(SizedBox(Some(6.0), None))),
                flexible(1, any(field)),
                inflexible(any(SizedBox(Some(8.0), None))),
                inflexible(any(Padding(EdgeInsets::symmetric(0.0, 6.0), send_btn))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )))
}

/// One composer affordance icon button (attach / emoji): a 44×44 [`fill_box`]
/// tile with a centered 24px icon (44 total, 24 glyph — down from the old
/// ~72px `filled_card` button). Uses the
/// Home tile idiom (`fill_box` disc + `SizedBox+Align` centered glyph) so the
/// tile is exactly [`COMPOSER_TILE`] rather than the card's 24-inset-driven
/// size; a headless test still locates its rounded chrome (the fill_box).
fn affordance_button<F>(leading: frust::IconSource, on_tap: F) -> AnyView<HuddleState>
where
    F: Fn(&mut HuddleState) + 'static,
{
    any(GestureDetector(Stack(vec![
        any(fill_box(
            Size::new(COMPOSER_TILE, COMPOSER_TILE),
            TILE_FILL,
            COMPOSER_TILE_RADIUS,
        )),
        any(SizedBox(Some(COMPOSER_TILE), Some(COMPOSER_TILE))
            .child(Align(Alignment::CENTER, icon(leading).size(24.0)))),
    ]))
    .on_tap(on_tap))
}

/// Send `text`: append it to the feed immediately, clear the composer, and
/// drive the canned reply on the background runtime. A no-op on empty/blank
/// input.
fn submit(controller: &Arc<MessagesController>, composer: RwSignal<String>, text: String) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    controller.send_now(text);
    composer.set(String::new());
    let controller = Arc::clone(controller);
    frust::spawn(async move {
        controller.deliver_reply().await;
    });
}
