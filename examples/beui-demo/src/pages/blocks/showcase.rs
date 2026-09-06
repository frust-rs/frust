//! Blocks · Showcase: the seven screen-scale blocks — the infinite masonry
//! feed, the project folder, the knockout bracket, the swap ticket, the
//! prediction market, the wallet card and the five 404 pages.
//!
//! Like the other Blocks pages this is a [`component`] over its own retained
//! [`State`]: every block here is controlled, and [`crate::AppState`] carries no
//! field for any of it.
//!
//! # The masonry gets a fixed-height box, and why
//!
//! `infinite_masonry` is a scroll surface in its own right — it clips a
//! viewport, owns the offset and consumes the wheel and drag itself — so it
//! needs a bounded height to window against. The gallery's page slot is a
//! scroll view, which hands its child an infinite max on the scroll axis, so
//! the feed is mounted inside a `SizedBox` that tightens the height. Scrolling
//! the feed and scrolling the page are therefore two separate gestures, which
//! is also what makes the append-on-approach visible at all: a feed whose items
//! all fit its viewport never scrolls and so never reports an approach.
//!
//! Everything the feed appends is synthesised here; a real caller would answer
//! `on_near_end` from its own data source and flip `loading` back when the
//! answer lands.

use frust_beui::blocks::infinite_masonry::{MasonryItem, infinite_masonry, masonry_item};
use frust_beui::blocks::knockout_bracket::{
    BracketMatch, BracketSide, BracketWinner, bracket_match, bracket_round, bracket_side,
    bracket_tbd, bracket_team, knockout_bracket,
};
use frust_beui::blocks::not_found::{NotFoundAction, NotFoundStyle, not_found};
use frust_beui::blocks::prediction_market::{
    MarketMode, MarketStatus, market_outcome, prediction_market,
};
use frust_beui::blocks::project_folder::{folder_preview, project_folder};
use frust_beui::blocks::swap::{SwapQuote, SwapToken, swap, swap_token};
use frust_beui::blocks::wallet_card::{WalletAction, wallet_account, wallet_card};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};

use frust::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, FlexView, SizedBox, TextView, any,
    component, inflexible, text,
};

use crate::AppState;

// ---- The page's own state --------------------------------------------------

/// How many cards each append adds, and the ceiling the feed stops at.
const MASONRY_PAGE: usize = 8;
/// The feed's ceiling — `has_more` clears here.
const MASONRY_MAX: usize = 48;

/// Everything the seven blocks on this page are driven by.
pub struct State {
    /// How many synthetic cards the feed currently holds.
    masonry_count: usize,
    folder_open: Option<bool>,
    folder_log: String,
    /// How many rounds of the tournament have been played, `0..=3`.
    bracket_played: usize,
    /// The bracket's leftmost visible round.
    bracket_page: usize,
    /// The swap's pay side; flipping exchanges the two.
    swap_from: SwapToken,
    swap_to: SwapToken,
    swap_amount: f64,
    swap_destination: String,
    swap_show_destination: bool,
    swap_log: String,
    /// The market's two outcome prices, nudged by the tick button.
    market_prices: [f64; 2],
    market_mode: MarketMode,
    market_outcome_id: String,
    market_amount: f64,
    market_status: MarketStatus,
    market_log: String,
    wallet_account_id: String,
    wallet_balance: f64,
    wallet_change: f64,
    wallet_log: String,
    not_found_style: NotFoundStyle,
    not_found_log: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            masonry_count: MASONRY_PAGE * 2,
            folder_open: None,
            folder_log: "(not opened yet)".to_string(),
            bracket_played: 1,
            bracket_page: 0,
            swap_from: eth(),
            swap_to: usdc(),
            swap_amount: 0.5,
            swap_destination: String::new(),
            swap_show_destination: false,
            swap_log: "(no swap submitted)".to_string(),
            market_prices: [0.167, 0.833],
            market_mode: MarketMode::Buy,
            market_outcome_id: "yes".to_string(),
            market_amount: 115.0,
            market_status: MarketStatus::Idle,
            market_log: "(no trade placed)".to_string(),
            wallet_account_id: "main".to_string(),
            wallet_balance: 12_480.32,
            wallet_change: 124.5,
            wallet_log: "(no action yet)".to_string(),
            not_found_style: NotFoundStyle::Glitch,
            not_found_log: "(no action yet)".to_string(),
        }
    }
}

// ---- Page chrome -----------------------------------------------------------

/// The page's own title.
fn heading(title: &str) -> TextView {
    text(title.to_string()).size(24.0)
}

/// A block's title.
fn section(title: &str) -> TextView {
    text(title.to_string()).size(16.0)
}

/// A block's small print.
fn caption(body: impl Into<String>) -> TextView {
    text(body.into()).size(12.0)
}

/// A vertical gap.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal gap.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// A centred row of controls.
fn controls(children: Vec<AnyView<State>>) -> AnyView<State> {
    let mut spaced = Vec::with_capacity(children.len() * 2);
    for (index, child) in children.into_iter().enumerate() {
        if index > 0 {
            spaced.push(hgap(8.0));
        }
        spaced.push(child);
    }
    any(FlexView::new(
        Axis::Horizontal,
        spaced.into_iter().map(inflexible).collect(),
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// A small secondary button.
fn minor(label: impl Into<String>, on_press: impl Fn(&mut State) + 'static) -> AnyView<State> {
    any(button(label, on_press)
        .tone(ButtonTone::Secondary)
        .size(ButtonSize::Sm))
}

// ---- The infinite masonry --------------------------------------------------

/// The feed's synthetic cards: a repeating category ladder with varying
/// extents, so the lanes actually pack unevenly.
fn masonry_items(count: usize) -> Vec<MasonryItem> {
    const CATEGORIES: [(&str, &str); 6] = [
        ("Soft geometry", "Architecture"),
        ("Open horizon", "Landscape"),
        ("Working rhythm", "Workspace"),
        ("Shared table", "Studio"),
        ("In session", "People"),
        ("After hours", "Office"),
    ];
    const EXTENTS: [f64; 6] = [260.0, 190.0, 230.0, 300.0, 210.0, 280.0];

    (0..count)
        .map(|index| {
            let (title, category) = CATEGORIES[index % CATEGORIES.len()];
            masonry_item(format!("{title} {}", index + 1))
                .caption(category)
                .extent(EXTENTS[index % EXTENTS.len()])
        })
        .collect()
}

/// The masonry block: a virtualized feed that asks for more as its viewport
/// nears the end.
fn masonry_block(state: &State) -> AnyView<State> {
    any(Column(vec![
        any(section("infinite_masonry")),
        gap(6.0),
        any(caption(
            "A responsive, virtualized masonry: cards are packed into shortest-lane columns and \
             only the windowed ones are shaped and painted. Scroll the feed \u{2014} it owns its \
             own offset \u{2014} and it appends another page once the viewport nears the end, \
             once per approach rather than once per frame.",
        )),
        gap(10.0),
        any(SizedBox(None, Some(420.0)).child(
            infinite_masonry(masonry_items(state.masonry_count), |s: &mut State| {
                if s.masonry_count >= MASONRY_MAX {
                    return;
                }
                // A real caller would go asking here and hold
                // `InfiniteMasonryView::loading` set until the answer landed,
                // which is also what unlatches the block's load gate; the
                // synthetic page below is immediate, so it never needs it.
                s.masonry_count = (s.masonry_count + MASONRY_PAGE).min(MASONRY_MAX);
            })
            .has_more(state.masonry_count < MASONRY_MAX)
            .end_label("That is every card.")
            .label("Gallery masonry feed"),
        )),
        gap(10.0),
        controls(vec![
            minor("Reset feed", |s: &mut State| {
                s.masonry_count = MASONRY_PAGE * 2;
            }),
            any(caption(format!(
                "{} of {MASONRY_MAX} cards",
                state.masonry_count
            ))),
        ]),
    ]))
}

// ---- The project folder ----------------------------------------------------

/// The folder block: a card whose preview sheets fan out on hover.
fn folder_block(state: &State) -> AnyView<State> {
    let previews = [
        "Moodboard",
        "Type study",
        "Palette",
        "Logo marks",
        "Layouts",
    ]
    .into_iter()
    .map(folder_preview)
    .collect();

    let mut folder = project_folder("Brand direction", previews)
        .description("Updated recently")
        .item_label("file")
        .count(5)
        .label("Brand direction folder")
        .on_open_change(|s: &mut State, open: bool| {
            s.folder_open = Some(open);
            s.folder_log = if open {
                "fanned open".to_string()
            } else {
                "closed".to_string()
            };
        })
        .on_activate(|s: &mut State| s.folder_log = "activated".to_string());
    if let Some(open) = state.folder_open {
        folder = folder.open(open);
    }

    any(Column(vec![
        any(section("project_folder")),
        gap(6.0),
        any(caption(
            "A folder card whose preview sheets fan out of it on hover or focus and tuck back \
             on leave. The fan is the block's own; pressing the card reports an activation.",
        )),
        gap(10.0),
        any(folder),
        gap(10.0),
        controls(vec![
            minor("Toggle open", |s: &mut State| {
                let next = !s.folder_open.unwrap_or(false);
                s.folder_open = Some(next);
            }),
            minor("Release to hover", |s: &mut State| s.folder_open = None),
            any(caption(state.folder_log.clone())),
        ]),
    ]))
}

// ---- The knockout bracket --------------------------------------------------

/// The quarter-finals: `(id, home, away, home score, away score)`. Every later
/// round is derived from these, so "advance" is one number.
const QUARTERS: [(&str, &str, &str, i64, i64); 4] = [
    ("qf-1", "Cloud9", "T1", 2, 1),
    ("qf-2", "G2 Esports", "Fnatic", 0, 3),
    ("qf-3", "Gen.G", "DRX", 3, 2),
    ("qf-4", "Team Liquid", "NAVI", 1, 2),
];

/// The winner and loser of one fixed quarter-final.
fn quarter_result(index: usize) -> (&'static str, &'static str) {
    let (_, home, away, home_score, away_score) = QUARTERS[index];
    if home_score >= away_score {
        (home, away)
    } else {
        (away, home)
    }
}

/// One side of a fixture: the named team with its score once the round has been
/// played, `TBD` before its feeders have.
fn side(name: Option<&str>, score: i64, played: bool) -> BracketSide {
    match name {
        Some(name) if played => bracket_side(bracket_team(name)).score(score),
        Some(name) => bracket_side(bracket_team(name)),
        None => bracket_tbd(),
    }
}

/// Assemble one fixture.
fn fixture(
    id: &str,
    date: &str,
    home: BracketSide,
    away: BracketSide,
    played: bool,
    winner: BracketWinner,
) -> BracketMatch {
    let card = bracket_match(id, home, away)
        .date(date)
        .time("20:00")
        .badge("BO5");
    if played {
        card.finished(true).winner(winner)
    } else {
        card
    }
}

/// Which side won, from a pair of scores.
fn winner_of(home_score: i64, away_score: i64) -> BracketWinner {
    if home_score >= away_score {
        BracketWinner::Home
    } else {
        BracketWinner::Away
    }
}

/// The bracket block: three rounds plus a play-off, advanced one round at a
/// time so the reveal animates.
fn bracket_block(state: &State) -> AnyView<State> {
    let played = state.bracket_played;

    let quarters: Vec<BracketMatch> = QUARTERS
        .iter()
        .map(|(id, home, away, home_score, away_score)| {
            fixture(
                id,
                "Sat, 14 Mar",
                side(Some(home), *home_score, played >= 1),
                side(Some(away), *away_score, played >= 1),
                played >= 1,
                winner_of(*home_score, *away_score),
            )
        })
        .collect();

    // Semi-finalists exist only once the quarters have been played.
    let semi_teams: Vec<Option<&'static str>> = if played >= 1 {
        (0..4).map(|index| Some(quarter_result(index).0)).collect()
    } else {
        vec![None; 4]
    };
    const SEMI_SCORES: [(i64, i64); 2] = [(3, 1), (2, 3)];
    let semis: Vec<BracketMatch> = (0..2)
        .map(|index| {
            let (home_score, away_score) = SEMI_SCORES[index];
            fixture(
                &format!("sf-{}", index + 1),
                "Wed, 18 Mar",
                side(semi_teams[index * 2], home_score, played >= 2),
                side(semi_teams[index * 2 + 1], away_score, played >= 2),
                played >= 2,
                winner_of(home_score, away_score),
            )
        })
        .collect();

    // Finalists — and the play-off pair — exist only once the semis have been.
    let (finalists, third_pair) = if played >= 2 {
        let first = (semi_teams[0], semi_teams[1]);
        let second = (semi_teams[2], semi_teams[3]);
        let pick = |(home, away): (Option<&'static str>, Option<&'static str>),
                    scores: (i64, i64)| {
            if scores.0 >= scores.1 {
                (home, away)
            } else {
                (away, home)
            }
        };
        let (w1, l1) = pick(first, SEMI_SCORES[0]);
        let (w2, l2) = pick(second, SEMI_SCORES[1]);
        ((w1, w2), (l1, l2))
    } else {
        ((None, None), (None, None))
    };

    const FINAL_SCORE: (i64, i64) = (3, 2);
    const THIRD_SCORE: (i64, i64) = (1, 2);
    let grand_final = fixture(
        "final",
        "Sun, 22 Mar",
        side(finalists.0, FINAL_SCORE.0, played >= 3),
        side(finalists.1, FINAL_SCORE.1, played >= 3),
        played >= 3,
        winner_of(FINAL_SCORE.0, FINAL_SCORE.1),
    );
    let third_place = fixture(
        "third",
        "Sun, 22 Mar",
        side(third_pair.0, THIRD_SCORE.0, played >= 3),
        side(third_pair.1, THIRD_SCORE.1, played >= 3),
        played >= 3,
        winner_of(THIRD_SCORE.0, THIRD_SCORE.1),
    );

    let rounds = vec![
        bracket_round("Quarter-finals", quarters),
        bracket_round("Semi-finals", semis),
        bracket_round("Grand final", vec![grand_final]),
    ];

    any(Column(vec![
        any(section("knockout_bracket")),
        gap(6.0),
        any(caption(
            "A single-elimination tree, widest round first, paged left and right with the \
             chevrons and joined by drawn connectors. Advance the tournament below: each round \
             fills its winners into the next, and the cards animate the change rather than \
             cutting to it.",
        )),
        gap(10.0),
        any(knockout_bracket(rounds)
            .page(state.bracket_page)
            .third_place(third_place)
            .third_place_label("Third place play-off")
            .label("Sample knockout bracket")
            .on_page_change(|s: &mut State, page: usize| s.bracket_page = page)),
        gap(10.0),
        controls(vec![
            minor("Advance round", |s: &mut State| {
                s.bracket_played = (s.bracket_played + 1).min(3);
            }),
            minor("Rewind round", |s: &mut State| {
                s.bracket_played = s.bracket_played.saturating_sub(1);
            }),
            any(caption(format!("{} of 3 rounds played", played))),
        ]),
    ]))
}

// ---- The swap --------------------------------------------------------------

/// The swap's pay-side token.
fn eth() -> SwapToken {
    swap_token("ETH", "Ethereum").balance(4.28).usd(3_120.0)
}

/// The swap's receive-side token.
fn usdc() -> SwapToken {
    swap_token("USDC", "Base").balance(2_140.0).usd(1.0)
}

/// The swap block: a two-chain ticket with a flip, a quote and an optional
/// destination row.
fn swap_block(state: &State) -> AnyView<State> {
    any(Column(vec![
        any(section("swap")),
        gap(6.0),
        any(caption(
            "A cross-chain swap ticket: the amount drives a live quote, the flip button turns the \
             pair over, and the destination row expands to an address field that validates as it \
             is typed. The action button says what it is refusing when it refuses.",
        )),
        gap(10.0),
        any(swap(state.swap_from.clone(), state.swap_to.clone())
            .amount(state.swap_amount)
            .destination(state.swap_destination.clone())
            .show_destination(state.swap_show_destination)
            .quote(SwapQuote::default())
            .eta("\u{2248} 24s")
            .title("Swap")
            .on_flip(|s: &mut State| {
                std::mem::swap(&mut s.swap_from, &mut s.swap_to);
                s.swap_log = "pair flipped".to_string();
            })
            .on_amount_change(|s: &mut State, amount: f64| s.swap_amount = amount)
            .on_destination_toggle(|s: &mut State, shown: bool| s.swap_show_destination = shown)
            .on_pick_token(|s: &mut State, side| s.swap_log = format!("token picker: {side:?}"))
            .on_submit(|s: &mut State| {
                s.swap_log = format!("swapped {} {}", s.swap_amount, s.swap_from.symbol());
            })),
        gap(8.0),
        any(caption(state.swap_log.clone())),
    ]))
}

// ---- The prediction market -------------------------------------------------

/// The market block: a buy/sell ticket whose prices tick under it.
fn market_block(state: &State) -> AnyView<State> {
    let outcomes = vec![
        market_outcome("yes", "Yes", state.market_prices[0]).position(125.0),
        market_outcome("no", "No", state.market_prices[1]).position(48.0),
    ];

    any(Column(vec![
        any(section("prediction_market")),
        gap(6.0),
        any(caption(
            "A buy/sell ticket over two outcomes: the price, the share count and the payout are \
             quoted from the amount, and a refusal shakes the ticket rather than silently doing \
             nothing. Tick the prices below \u{2014} the numbers roll to their new value instead \
             of snapping.",
        )),
        gap(10.0),
        any(prediction_market(outcomes)
            .mode(state.market_mode)
            .outcome_id(state.market_outcome_id.clone())
            .amount(state.market_amount)
            .balance(500.0)
            .min_trade(1.0)
            .quick_amounts(vec![1.0, 5.0, 10.0, 100.0])
            .order_type_label("Market")
            .status(state.market_status)
            .authenticated(true)
            .on_mode_change(|s: &mut State, mode: MarketMode| s.market_mode = mode)
            .on_outcome_change(|s: &mut State, id: String| s.market_outcome_id = id)
            .on_amount_change(|s: &mut State, amount: f64| s.market_amount = amount)
            .on_trade(|s: &mut State| {
                s.market_status = MarketStatus::Filled;
                s.market_log = format!(
                    "{} {} at {:.0}c",
                    if s.market_mode == MarketMode::Buy {
                        "bought"
                    } else {
                        "sold"
                    },
                    s.market_outcome_id,
                    s.market_prices[usize::from(s.market_outcome_id == "no")] * 100.0
                );
            })),
        gap(10.0),
        controls(vec![
            minor("Tick prices", |s: &mut State| {
                // A market feed's next print, stood in for by a fixed nudge:
                // deterministic, and enough to show the ticker roll.
                let next = (s.market_prices[0] + 0.037).rem_euclid(0.94).max(0.03);
                s.market_prices = [next, 1.0 - next];
                s.market_status = MarketStatus::Idle;
            }),
            minor("Reset ticket", |s: &mut State| {
                s.market_prices = [0.167, 0.833];
                s.market_amount = 115.0;
                s.market_status = MarketStatus::Idle;
                s.market_log = "(no trade placed)".to_string();
            }),
            any(caption(state.market_log.clone())),
        ]),
    ]))
}

// ---- The wallet card -------------------------------------------------------

/// The wallet block: a balance card with an account switcher and a search
/// panel that morph into the same box.
fn wallet_block(state: &State) -> AnyView<State> {
    let accounts = vec![
        wallet_account(
            "main",
            "Main Wallet",
            "0x8f3Cb1a29e4D7c6F1B2a3E9d0C4b5A6f7D8e9C0b",
        ),
        wallet_account(
            "trading",
            "Trading",
            "0x1a2B3c4D5e6F7a8B9c0D1e2F3a4B5c6D7e8F9a0B",
        ),
        wallet_account(
            "cold",
            "Cold Storage",
            "0x9F8e7D6c5B4a3E2d1C0b9A8f7E6d5C4b3A2e1F0d",
        ),
    ];

    any(Column(vec![
        any(section("wallet_card")),
        gap(6.0),
        any(caption(
            "A balance card whose header row morphs between the account switcher and a recent-\
             search picker \u{2014} the two share one box, so only one is ever open. The balance \
             rolls to a new value, and the four actions report which was pressed.",
        )),
        gap(10.0),
        any(wallet_card(accounts, state.wallet_balance)
            .account_id(state.wallet_account_id.clone())
            .balance_prefix("$")
            .change(state.wallet_change)
            .has_notifications(true)
            .search_recent(vec![
                "vitalik.eth".to_string(),
                "0xA0b8\u{2026}6EB4".to_string(),
                "Uniswap".to_string(),
                "Send to Trading".to_string(),
            ])
            .on_account_change(|s: &mut State, id: String| {
                s.wallet_log = format!("switched to {id}");
                s.wallet_account_id = id;
            })
            .on_action(|s: &mut State, action: WalletAction| {
                s.wallet_log = format!("{} pressed", action.label());
            })
            .on_search_submit(|s: &mut State, query: String| {
                s.wallet_log = format!("searched {query}");
            })
            .on_copy_address(|s: &mut State, address: String| {
                s.wallet_log = format!("copied {address}");
            })
            .on_notifications(|s: &mut State| {
                s.wallet_log = "notifications opened".to_string();
            })),
        gap(10.0),
        controls(vec![
            minor("Simulate balance change", |s: &mut State| {
                // A deterministic stand-in for a chain update: alternate a
                // credit and a debit so the ticker rolls both ways.
                let up = s.wallet_change <= 0.0;
                let delta = if up { 268.4 } else { -142.9 };
                s.wallet_balance = (s.wallet_balance + delta).max(0.0);
                s.wallet_change = delta;
            }),
            any(caption(state.wallet_log.clone())),
        ]),
    ]))
}

// ---- The 404 pages ---------------------------------------------------------

/// The style's own name, for the picker.
fn style_label(style: NotFoundStyle) -> &'static str {
    match style {
        NotFoundStyle::Glitch => "Glitch",
        NotFoundStyle::Magnetic => "Magnetic",
        NotFoundStyle::Spotlight => "Spotlight",
        NotFoundStyle::Stacked => "Stacked",
        NotFoundStyle::Terminal => "Terminal",
    }
}

/// The 404 block: one component with five presentations, switched below it.
fn not_found_block(state: &State) -> AnyView<State> {
    let picker: Vec<AnyView<State>> = NotFoundStyle::ALL
        .into_iter()
        .map(|style| {
            let selected = style == state.not_found_style;
            any(button(style_label(style), move |s: &mut State| {
                s.not_found_style = style;
            })
            .tone(if selected {
                ButtonTone::Primary
            } else {
                ButtonTone::Outline
            })
            .size(ButtonSize::Sm))
        })
        .collect();

    any(Column(vec![
        any(section("not_found")),
        gap(6.0),
        any(caption(
            "Five 404 presentations behind one block: the digits scramble, the digits chase the \
             cursor, a spotlight reveals them, a card deck fans, or a terminal types a failed \
             `cd`. Two of them track the pointer inside their stage and two paint their own dark \
             panel.",
        )),
        gap(10.0),
        controls(picker),
        gap(12.0),
        any(not_found::<State>()
            .style(state.not_found_style)
            .code("404")
            .title("Page not found")
            .description("The page you were looking for has moved or never existed.")
            .home_label("Back home")
            .browse_label("Browse components")
            .on_action(|s: &mut State, action: NotFoundAction| {
                s.not_found_log = match action {
                    NotFoundAction::Home => "back home".to_string(),
                    NotFoundAction::Browse => "browse components".to_string(),
                };
            })),
        gap(8.0),
        any(caption(format!("Last action: {}", state.not_found_log))),
    ]))
}

// ---- The page --------------------------------------------------------------

/// The Blocks · Showcase page.
#[derive(Default)]
struct ShowcasePage;

impl Component for ShowcasePage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> AnyView<State> {
        any(Column(vec![
            any(heading("Blocks \u{b7} Showcase")),
            gap(8.0),
            any(caption(
                "Seven screen-scale blocks \u{2014} a feed, a folder, a tournament, two finance \
                 tickets, a wallet and five 404 pages \u{2014} over sample data held here on the \
                 page.",
            )),
            gap(24.0),
            masonry_block(state),
            gap(24.0),
            folder_block(state),
            gap(24.0),
            bracket_block(state),
            gap(24.0),
            swap_block(state),
            gap(24.0),
            market_block(state),
            gap(24.0),
            wallet_block(state),
            gap(24.0),
            not_found_block(state),
        ]))
    }
}

/// The Blocks · Showcase page, hosted over its own retained [`State`].
pub fn page() -> AnyView<AppState> {
    any(component(ShowcasePage))
}
