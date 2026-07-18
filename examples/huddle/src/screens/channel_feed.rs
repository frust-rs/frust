//! Channel / DM message feed (`/channel/:id`).
//!
//! The feed screen: message bubbles (own messages right-aligned, others left
//! with an avatar + name), link-preview and file-stub cards, reaction chips,
//! a thread affordance, a typing indicator, loading skeletons, near-start
//! pagination on `#firehose`, and the expanding multiline composer. All state
//! lives in the [`MessagesController`](crate::features::messages) this
//! screen sources via
//! [`MessagesController::for_channel`](crate::features::messages::MessagesController::for_channel)
//! — the shared per-channel-id registry (task 19) that lets a reply composed
//! in an open [`crate::screens::thread`] update this same screen's rendered
//! reply count, and vice versa.
//!
//! # Why a plain function, not a `Component` (mirrors `screens::thread`)
//!
//! An earlier version of this screen hosted its controller behind a
//! `Component`'s retained state (via `clean_signals_forgekit::use_controller`).
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
//! # Two deliberate adaptations to the facade-only widget set
//!
//! This example crate depends on the `forgekit` facade **alone** (see
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions), so a couple of
//! the task's visual items are expressed with the widgets the facade ships
//! rather than a bespoke `Widget`:
//!
//! - **Feed container.** The task named `ListView` + `on_near_start`, but
//!   `ListView` requires a single uniform `item_extent` ("variable-extent lazy
//!   layout is deferred" — its own module docs) and reconciles by raw index,
//!   which cannot render variable-height chat bubbles nor preserve the keyed
//!   identity the entrance item asks for. The feed is instead a
//!   [`scroll_view`](forgekit::scroll_view) over a [`keyed`](forgekit::keyed)
//!   [`Column`](forgekit::Column) — variable heights + stable per-message keys
//!   — with the same near-start "load older" trigger driven off
//!   [`ScrollView::on_scroll`](forgekit::ScrollView) instead of
//!   `on_near_start`.
//! - **Per-message entrance + typing indicator.** A per-item slide/fade
//!   `AnimationController` needs a custom painting `Widget`, which a facade-only
//!   crate cannot author (the same limitation the `ui::toast` module already
//!   notes). The keyed list gives newly-appended messages stable identity, and
//!   the "being typed" affordance uses the facade's self-animating
//!   [`loading_indicator`](forgekit::loading_indicator) /
//!   [`cupertino_activity_indicator`](forgekit::cupertino_activity_indicator)
//!   as a live animation. Bubble backgrounds use `Card` variants (own =
//!   `filled_card`) rather than a `primary_container` tint, since the facade
//!   exposes no color-scheme roles or explicit fill color to app code.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use forgekit::{
    AnyView, Axis, CrossAxisAlignment, DesignLanguage, EdgeInsets, FlexView, Get, GetUntracked,
    MainAxisAlignment, NavigatorController, Padding, RwSignal, ScrollInfo, Set, SizedBox, Theme,
    any, app_bar, assist_chip, cupertino_activity_indicator, elevated_card, filled_card,
    filter_chip, flexible, icon, icons, inflexible, keyed, loading_indicator, outlined_card,
    scroll_view, text, text_input, use_context,
};

use crate::HuddleState;
use crate::features::messages::{FeedBody, FeedMessage, MessagesController};
use crate::mock;

/// Distance (logical px) from the top of the feed at which the near-start
/// "load older" trigger fires — the infinite-scroll edge for `#firehose`.
const NEAR_START_PX: f64 = 96.0;

/// Uniform row height for a loading skeleton bar.
const SKELETON_HEIGHT: f64 = 44.0;
/// How many skeleton bars the loading state shows.
const SKELETON_COUNT: usize = 6;

/// A quick-react emoji offered under every bubble (the "add reaction"
/// affordance). Real color-glyph rendering, verified in wave A.
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
        *cell
            .borrow_mut()
            .entry(channel_id.to_string())
            .or_insert_with(|| RwSignal::new(String::new()))
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
    let design = use_context::<Theme>()
        .map(|t| t.design_language)
        .unwrap_or(DesignLanguage::Material3);

    let bar = feed_app_bar(&channel_id, navigator);
    let body = feed_body(&controller, design);
    let composer_row = composer_bar(&controller, composer);

    any(FlexView::new(
        Axis::Vertical,
        vec![inflexible(bar), flexible(1, body), inflexible(composer_row)],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}

/// The app bar: a back button, the channel/DM title, and a member-count
/// subtitle folded into the title (the M3 app bar has one title slot).
fn feed_app_bar(
    channel_id: &str,
    navigator: NavigatorController<HuddleState>,
) -> AnyView<HuddleState> {
    let title = if let Some(ch) = mock::channel(channel_id) {
        let members = mock::users().len();
        format!("#{}  ·  {members} members", ch.name)
    } else if let Some(peer) = mock::dms()
        .iter()
        .find(|d| d.id == channel_id)
        .and_then(|d| mock::user(d.user_id))
    {
        format!("{}  ·  {}", peer.name, peer.status.label())
    } else {
        channel_id.to_string()
    };

    let back = forgekit::GestureDetector(icon(icons::ARROW_BACK).size(24.0))
        .on_tap(move |_st: &mut HuddleState| navigator.pop());

    any(app_bar::<HuddleState>(title).leading(any(Padding(EdgeInsets::symmetric(4.0, 0.0), back))))
}

/// The scrolling feed area: skeletons while loading, else the keyed message
/// column with the near-start pagination trigger and a trailing typing row.
fn feed_body(controller: &Arc<MessagesController>, design: DesignLanguage) -> AnyView<HuddleState> {
    if controller.loading.get() {
        return skeletons();
    }

    let messages = controller.feed(); // tracked
    let loading_older = controller.loading_older.get();
    let typing = controller.typing.get();

    let mut children: Vec<forgekit::FlexChild<HuddleState>> = Vec::new();

    if loading_older {
        children.push(inflexible(loading_older_row(design)));
    }

    for msg in &messages {
        children.push(keyed(msg.id, message_row(controller, msg)));
    }

    if typing {
        children.push(inflexible(typing_row(design)));
    }

    let column = FlexView::new(Axis::Vertical, children)
        .cross_axis(CrossAxisAlignment::Stretch)
        .main_axis(MainAxisAlignment::Start);

    let pager = Arc::clone(controller);
    any(
        scroll_view(Padding(EdgeInsets::all(12.0), column)).on_scroll(
            move |_st: &mut HuddleState, info: ScrollInfo| {
                if info.offset <= NEAR_START_PX
                    && pager.has_more.get_untracked()
                    && !pager.loading_older.get_untracked()
                {
                    let pager = Arc::clone(&pager);
                    forgekit::spawn(async move {
                        pager.load_older().await;
                    });
                }
            },
        ),
    )
}

/// One message row: the bubble, aligned right (own) or left (others, with an
/// avatar), inside horizontal breathing room.
fn message_row(controller: &Arc<MessagesController>, msg: &FeedMessage) -> AnyView<HuddleState> {
    let bubble = message_bubble(controller, msg);
    let row = if msg.is_own() {
        FlexView::new(
            Axis::Horizontal,
            vec![flexible(1, any(SizedBox(None, None))), inflexible(bubble)],
        )
    } else {
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(avatar(msg.author_id)),
                inflexible(any(SizedBox(Some(8.0), None))),
                inflexible(bubble),
                flexible(1, any(SizedBox(None, None))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start)
    };
    any(Padding(EdgeInsets::symmetric(0.0, 4.0), row))
}

/// The bubble card: author line (others only), body, reaction chips, and the
/// thread affordance.
fn message_bubble(controller: &Arc<MessagesController>, msg: &FeedMessage) -> AnyView<HuddleState> {
    let mut lines: Vec<AnyView<HuddleState>> = Vec::new();

    if !msg.is_own()
        && let Some(user) = mock::user(msg.author_id)
    {
        lines.push(any(text(format!("{}  ·  9:41 AM", user.name)).size(12.0)));
    }

    lines.push(message_body(msg));

    let reactions = reaction_chips(controller, msg);
    if let Some(reactions) = reactions {
        lines.push(reactions);
    }

    if msg.reply_count() > 0 {
        lines.push(thread_affordance(msg));
    }

    let inner = Padding(
        EdgeInsets::symmetric(12.0, 8.0),
        FlexView::new(
            Axis::Vertical,
            lines.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Start),
    );

    if msg.is_own() {
        any(filled_card(inner))
    } else {
        any(elevated_card(inner))
    }
}

/// The message body: plain text, a link-preview card, or a file-stub card.
fn message_body(msg: &FeedMessage) -> AnyView<HuddleState> {
    match &msg.body {
        FeedBody::Text(t) => any(text(t.clone()).size(15.0)),
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

    let mut chips: Vec<forgekit::FlexChild<HuddleState>> = Vec::new();
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

/// A small avatar carrying the author's initials (the facade exposes no
/// circular clip to app code, so a `filled_card` stands in for the disc).
fn avatar(author_id: u32) -> AnyView<HuddleState> {
    let initials = mock::user(author_id).map(|u| u.initials).unwrap_or("?");
    any(filled_card(Padding(
        EdgeInsets::all(8.0),
        text(initials.to_string()).size(12.0),
    )))
}

/// The typing indicator row (a self-animating facade spinner + label).
fn typing_row(design: DesignLanguage) -> AnyView<HuddleState> {
    let spinner: AnyView<HuddleState> = match design {
        DesignLanguage::Cupertino => any(cupertino_activity_indicator()),
        DesignLanguage::Material3 => any(loading_indicator()),
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
        DesignLanguage::Material3 => any(loading_indicator()),
    };
    any(Padding(
        EdgeInsets::all(8.0),
        forgekit::Align(
            forgekit::Alignment::CENTER,
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

/// The loading skeleton: a column of grey placeholder bars.
fn skeletons() -> AnyView<HuddleState> {
    let mut rows: Vec<AnyView<HuddleState>> = Vec::new();
    for _ in 0..SKELETON_COUNT {
        rows.push(any(Padding(
            EdgeInsets::all(8.0),
            filled_card(SizedBox(None, Some(SKELETON_HEIGHT))),
        )));
    }
    any(Padding(
        EdgeInsets::all(12.0),
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
) -> AnyView<HuddleState> {
    let value = composer.get(); // tracked
    let has_text = !value.trim().is_empty();

    let mut column: Vec<AnyView<HuddleState>> = Vec::new();

    if has_text {
        column.push(affordance_chips());
    }

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
        any(
            forgekit::GestureDetector(send_icon).on_tap(move |_st: &mut HuddleState| {
                let text = composer.get_untracked();
                submit(&send_ctrl, composer, text);
            }),
        )
    } else {
        // Empty: the button is inert (no handler), just the glyph.
        any(send_icon)
    };

    column.push(any(FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(1, any(field)),
            inflexible(any(SizedBox(Some(8.0), None))),
            inflexible(any(Padding(EdgeInsets::symmetric(0.0, 6.0), send_btn))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center)));

    any(Padding(
        EdgeInsets::all(8.0),
        FlexView::new(
            Axis::Vertical,
            column.into_iter().map(inflexible).collect::<Vec<_>>(),
        )
        .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

/// The attachment / emoji affordance chips above the field. Inert for now (the
/// pickers are Phase D sheets); they carry only a pressed state.
fn affordance_chips() -> AnyView<HuddleState> {
    any(Padding(
        EdgeInsets::symmetric(0.0, 6.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(assist_chip::<HuddleState, _>(
                    "Attach",
                    |_st: &mut HuddleState| {},
                )
                .leading("\u{1F4CE}"))),
                inflexible(any(SizedBox(Some(8.0), None))),
                inflexible(any(assist_chip::<HuddleState, _>(
                    "Emoji",
                    |_st: &mut HuddleState| {},
                )
                .leading("\u{1F642}"))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ))
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
    forgekit::spawn(async move {
        controller.deliver_reply().await;
    });
}
