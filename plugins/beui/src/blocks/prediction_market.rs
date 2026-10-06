//! Ports beUI's `prediction-market` block —
//! `components/motion/prediction-market.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `prediction-market`: *"Prediction market trade ticket with buy/sell
//! modes, outcome prices, rolling amount entry, quick add chips and trade
//! states."*
//!
//! # A premise correction
//!
//! The porting card describes this component as having *"probability bar
//! animations"*. Upstream has no probability bar: the outcome prices are shown
//! as **pill tabs** (`Up 9¢` / `Down 91¢`), tinted by outcome and selected one
//! at a time, and the only animated number is the payout ticker in the footer.
//! The port follows upstream; a bar would be a design this component does not
//! have. The ticker half of the card's note is real and is carried — see below.
//!
//! | upstream | here |
//! |---|---|
//! | `max-w-[400px] rounded-3xl border bg-background` | [`MARKET_WIDTH`], [`MARKET_RADIUS`] |
//! | `MODES` (`Buy` / `Sell`) on the underline `Tabs` | [`MarketMode`], [`MARKET_UNDERLINE_HEIGHT`] |
//! | `DEFAULT_OUTCOMES` and their `formatCents` labels | [`MarketOutcome`], [`format_cents`] |
//! | the `h-14` pill `TabsList`, emerald/red by outcome | [`MARKET_OUTCOME_HEIGHT`], [`MarketOutcomeTone`] |
//! | `AnimatedAmountInput`'s per-character `{opacity, y: 18 → 0}` | [`MARKET_DIGIT_TRAVEL`], [`MARKET_DIGIT_TRANSITION`] |
//! | `amountInputSize`'s four type rungs | [`market_amount_size`] |
//! | `DEFAULT_QUICK_AMOUNTS` plus `Max` | [`MARKET_QUICK_AMOUNTS`] |
//! | `buildQuote`'s five branches | [`market_quote`], [`MarketQuote`] |
//! | the `x: [0, -5, 5, -3, 3, -1, 0]` refusal shake over `0.38s` | [`MARKET_SHAKE`], [`MARKET_SHAKE_FRAMES`] |
//! | `NumberTicker` on the payout | [`crate::components::number`], hosted as a child |
//! | `StatefulButton`'s idle / loading / success / error arms | [`MarketStatus`] |
//!
//! # The payout ticker is the catalog's own number
//!
//! The card asks for the price tick to *"reuse `components::number` via the
//! crate API where practical"*, and it is practical: the footer's payout is a
//! [`crate::components::number`] child pod in
//! [`NumberMode::Roll`], driven by
//! the quote, with the `$` supplied as its prefix. That is the whole of
//! upstream's `NumberTicker` usage here (`startOnView={false}`, `stagger={0}`,
//! `format` to a currency), so the port is the component rather than a
//! re-implementation of it.
//!
//! `stagger={0}` is the one detail that does not survive: `NumberView` exposes
//! [`stagger`](crate::components::number::NumberView::stagger), so it is passed
//! as [`Duration::ZERO`] and the columns move together, exactly as upstream's
//! do.
//!
//! # Degradations against the web original
//!
//! - **No editable amount field.** The amount is an `<input>` upstream; here it
//!   is the app's own number ([`PredictionMarketView::amount`]), and every
//!   *other* way it changes — the quick-add chips, `Max`, and a mode switch
//!   clearing it — is carried and reported through
//!   [`PredictionMarketView::on_amount_change`]. The per-character entry
//!   animation is kept: the digits still roll in as the value changes.
//! - **No order-type menu.** Upstream's `Market ⌄` button opens nothing — it is
//!   chrome — so it is painted and inert here too.
//! - **No blur on the digit swap.** `filter: blur(10px)` has no primitive; the
//!   opacity and travel halves are kept.
//! - **The outcome hues are beUI's own.** Upstream hardcodes Tailwind's
//!   `emerald-500`/`red-500`; this port reads [`crate::BeuiTokens`]' authored
//!   `--success`/`--danger`, the substitution
//!   [`crate::components::animated_badge`] records for the same reason.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, Color,
    ErasedArgCallback, ErasedCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx,
    PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2, View,
    Widget, any, build_child, erase_callback, erase_callback_arg, rebuild_child,
    route_event_single, teardown_child, text::TextStyle, visit_children,
};
use frust::{FrameTime, Theme};

use crate::components::number::{NumberMode, number};
use crate::components::popover::{PanelChrome, paint_panel_hairline, resolve_panel};
use crate::motion::Ramp;
use crate::press::{Lane, inside, is_activation_key, keyframes_at, press_scale, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::{EASE_OUT, SPRING_SWAP};

// ---- Metrics ---------------------------------------------------------------

/// `max-w-[400px]` — the ticket's width, in logical px.
pub const MARKET_WIDTH: f64 = 400.0;

/// `rounded-3xl` — its corner radius.
pub const MARKET_RADIUS: f64 = style::RADIUS_3XL;

/// `px-4 pt-4` — the header's padding, in logical px.
pub const MARKET_PADDING: f64 = 16.0;

/// `p-3` — the body's padding, in logical px.
pub const MARKET_BODY_PADDING: f64 = 12.0;

/// `text-2xl` — a mode tab's type size, in logical px.
pub const MARKET_MODE_SIZE: f64 = 24.0;

/// `pb-3` — the gap under a mode tab before its indicator.
pub const MARKET_MODE_GAP: f64 = 12.0;

/// `gap-5` — the gap between the two mode tabs.
pub const MARKET_MODE_SPACING: f64 = 20.0;

/// `h-0.5` — the selected mode's underline, in logical px.
pub const MARKET_UNDERLINE_HEIGHT: f64 = 2.0;

/// `h-14` — an outcome pill's height, in logical px.
pub const MARKET_OUTCOME_HEIGHT: f64 = 56.0;

/// `rounded-[1.35rem]` — its corner radius, in logical px.
pub const MARKET_OUTCOME_RADIUS: f64 = 21.6;

/// `gap-2 p-1.5` — the gap between outcome pills and the tray's own padding.
pub const MARKET_OUTCOME_GAP: f64 = 8.0;

/// The amount card's own padding, in logical px (`rounded-3xl bg-card p-4`).
pub const MARKET_AMOUNT_PADDING: f64 = 16.0;

/// `min-h-24` — the amount block's least height, in logical px.
pub const MARKET_AMOUNT_MIN_HEIGHT: f64 = 96.0;

/// `h-9 rounded-xl px-3.5` — a quick-add chip's box.
pub const MARKET_CHIP_HEIGHT: f64 = 36.0;

/// Its horizontal padding, in logical px.
pub const MARKET_CHIP_PADDING_X: f64 = 14.0;

/// `gap-2` — the gap between chips.
pub const MARKET_CHIP_GAP: f64 = 8.0;

/// `h-12 rounded-2xl` — the action button's box.
pub const MARKET_ACTION_HEIGHT: f64 = 48.0;

/// `DEFAULT_QUICK_AMOUNTS` — the four chips before `Max`.
pub const MARKET_QUICK_AMOUNTS: [f64; 4] = [10.0, 50.0, 100.0, 500.0];

/// `minTrade = 1` — the least a buy may be.
pub const MARKET_MIN_TRADE: f64 = 1.0;

/// `balance = 500` — the wallet the demo starts with.
pub const MARKET_DEFAULT_BALANCE: f64 = 500.0;

/// `bg-emerald-500/20` and `bg-red-500/10` — an outcome pill's fill alpha when
/// it is selected.
pub const MARKET_OUTCOME_FILL_ALPHA: f32 = 0.18;

/// `disabled:opacity-50` — a disabled chip's ink opacity.
pub const MARKET_DISABLED_OPACITY: f32 = 0.5;

// ---- Motion ----------------------------------------------------------------

/// `DIGIT_TRANSITION` — `{ duration: 0.18, ease: EASE_OUT }`, what one entering
/// or leaving character of the amount plays.
pub const MARKET_DIGIT_TRANSITION: Duration = Duration::from_millis(180);

/// `initial={{ y: 18 }}` — how far below its slot an entering character starts,
/// in logical px.
pub const MARKET_DIGIT_TRAVEL: f64 = 18.0;

/// `exit={{ y: -14 }}` — how far above it a leaving one goes.
pub const MARKET_DIGIT_EXIT_TRAVEL: f64 = -14.0;

/// `duration: 0.38` — how long the refusal shake takes.
pub const MARKET_SHAKE: Duration = Duration::from_millis(380);

/// `x: [0, -5, 5, -3, 3, -1, 0]` — the shake's own keyframes, in logical px.
pub const MARKET_SHAKE_FRAMES: [f64; 7] = [0.0, -5.0, 5.0, -3.0, 3.0, -1.0, 0.0];

/// `setTimeout(…, 650)` — how long a placed trade spends filling.
pub const MARKET_PLACING: Duration = Duration::from_millis(650);

/// `active:scale-95` — a chip's press shrink.
pub const MARKET_CHIP_PRESS_SCALE: f64 = 0.95;

/// `pressScale={0.98}` — the action button's.
pub const MARKET_ACTION_PRESS_SCALE: f64 = 0.98;

/// `duration: 0.45` — the payout ticker's own roll.
pub const MARKET_TICKER: Duration = Duration::from_millis(450);

// ---- Formatting -------------------------------------------------------------

/// `formatCents`: a `0..=1` price as a cents label — `9¢`, `91¢`, `9.5¢`.
///
/// A whole number of cents drops its decimal; anything else keeps one, which is
/// upstream's `Number.isInteger(cents) ? 0 : 1`.
pub fn format_cents(price: f64) -> String {
    let cents = price * 100.0;
    if (cents - cents.round()).abs() < f64::EPSILON {
        format!("{}¢", cents.round() as i64)
    } else {
        format!("{cents:.1}¢")
    }
}

/// `formatCurrency`: a USD amount at `digits` fraction digits, thousands
/// grouped.
///
/// The separator is an ASCII comma for the reason
/// [`crate::components::number`] records: the facade publishes no locale seam.
pub fn format_currency(amount: f64, digits: usize) -> String {
    let fixed = format!("{:.*}", digits, amount.abs());
    let (whole, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), ""));
    let mut grouped = String::new();
    for (index, ch) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let sign = if amount < 0.0 { "-" } else { "" };
    if fraction.is_empty() {
        format!("{sign}${grouped}")
    } else {
        format!("{sign}${grouped}.{fraction}")
    }
}

/// `formatCompactCurrency`: whole dollars from 100 up, and below that only as
/// many decimals as the value actually has.
pub fn format_compact_currency(amount: f64) -> String {
    // Upstream reaches whole dollars two ways — from 100 up, and for any
    // whole value below it — and the two branches happen to agree; kept as one
    // test rather than restated, since the intents are the same digit count.
    if amount >= 100.0 || amount.fract() == 0.0 {
        format_currency(amount, 0)
    } else {
        format_currency(amount, 2)
    }
}

/// `amountInputSize`: the type size the amount is drawn at, given how many
/// digits it has — upstream's four rungs, resolved to logical px.
///
/// The responsive halves of each rung (`sm:text-*`) are the larger of the pair,
/// since this card has one width rather than a breakpoint.
pub fn market_amount_size(digits: usize) -> f64 {
    if digits >= 10 {
        36.0
    } else if digits >= 8 {
        48.0
    } else if digits >= 6 {
        56.0
    } else {
        60.0
    }
}

// ---- The order and its quote -----------------------------------------------

/// Which side of the market an order is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketMode {
    /// Buying shares with cash.
    #[default]
    Buy,
    /// Selling shares back.
    Sell,
}

impl MarketMode {
    /// Both, in upstream's own tab order.
    pub const ALL: [MarketMode; 2] = [MarketMode::Buy, MarketMode::Sell];

    /// The tab's label.
    pub const fn label(self) -> &'static str {
        match self {
            MarketMode::Buy => "Buy",
            MarketMode::Sell => "Sell",
        }
    }

    /// What the amount block's own label calls the quantity.
    pub const fn amount_label(self) -> &'static str {
        match self {
            MarketMode::Buy => "Amount",
            MarketMode::Sell => "Shares",
        }
    }

    /// What the footer calls the payout.
    pub const fn payout_label(self) -> &'static str {
        match self {
            MarketMode::Buy => "To win",
            MarketMode::Sell => "To receive",
        }
    }
}

/// Which way an outcome pill is tinted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketOutcomeTone {
    /// The affirmative outcome: the success hue.
    Yes,
    /// The negative one: the danger hue.
    No,
}

impl MarketOutcomeTone {
    /// Upstream's own test: an outcome labelled `no` or `down` is the negative
    /// one, and everything else is affirmative. Case-insensitive, as its
    /// `toLowerCase()` is.
    pub fn of(label: &str) -> Self {
        let lower = label.to_lowercase();
        if lower == "no" || lower == "down" {
            MarketOutcomeTone::No
        } else {
            MarketOutcomeTone::Yes
        }
    }
}

/// One outcome the market offers.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketOutcome {
    id: String,
    label: String,
    price: f64,
    position: f64,
}

/// An outcome with `id`, shown as `label`, trading at `price` (in `0..=1`).
pub fn market_outcome(
    id: impl Into<String>,
    label: impl Into<String>,
    price: f64,
) -> MarketOutcome {
    MarketOutcome {
        id: id.into(),
        label: label.into(),
        price,
        position: 0.0,
    }
}

impl MarketOutcome {
    /// Set how many shares of this outcome the wallet already holds
    /// (upstream's `positions` map).
    pub fn position(mut self, position: f64) -> Self {
        self.position = position;
        self
    }

    /// The outcome's stable id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Its display label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Its price, in `0..=1`.
    pub fn price(&self) -> f64 {
        self.price
    }

    /// The wallet's holding of it.
    pub fn holding(&self) -> f64 {
        self.position
    }

    /// Which way its pill is tinted.
    pub fn tone(&self) -> MarketOutcomeTone {
        MarketOutcomeTone::of(&self.label)
    }
}

/// What an order works out to — upstream's `PredictionMarketQuote`.
#[derive(Clone, Debug, PartialEq)]
pub struct MarketQuote {
    /// Whether the order may be placed.
    pub valid: bool,
    /// The amount it was quoted at.
    pub amount: f64,
    /// The price it clears at, clamped into `0.01..=0.99`.
    pub price: f64,
    /// How many shares it moves.
    pub shares: f64,
    /// What it pays out.
    pub payout: f64,
    /// Why it cannot be placed, when it cannot.
    pub error: Option<String>,
}

/// `buildQuote`: what `amount` works out to against `outcome`, given the
/// wallet's `balance` and its holding of that outcome.
///
/// The five branches are upstream's own, in its own order: an empty amount, a
/// buy under the minimum, a buy over the balance, a sell over the holding, and
/// finally a valid order. Every branch still reports the arithmetic, so an
/// invalid ticket still shows what it *would* have paid.
///
/// The price is clamped into `0.01..=0.99` before anything divides by it, which
/// is what keeps a degenerate market from quoting an infinite share count.
pub fn market_quote(
    mode: MarketMode,
    amount: f64,
    outcome: &MarketOutcome,
    balance: f64,
    min_trade: f64,
) -> MarketQuote {
    let price = outcome.price.clamp(0.01, 0.99);
    let shares = match mode {
        MarketMode::Buy => amount / price,
        MarketMode::Sell => amount,
    };
    let payout = match mode {
        MarketMode::Buy => shares,
        MarketMode::Sell => amount * price,
    };
    let refuse = |error: String, shares: f64, payout: f64| MarketQuote {
        valid: false,
        amount,
        price,
        shares,
        payout,
        error: Some(error),
    };

    if amount <= 0.0 {
        return refuse("Enter an amount".to_string(), 0.0, 0.0);
    }
    if mode == MarketMode::Buy && amount < min_trade {
        return refuse(
            format!("Minimum {}", format_compact_currency(min_trade)),
            shares,
            payout,
        );
    }
    if mode == MarketMode::Buy && amount > balance {
        return refuse("Insufficient balance".to_string(), shares, payout);
    }
    if mode == MarketMode::Sell && amount > outcome.position {
        return refuse("Not enough shares".to_string(), shares, payout);
    }
    MarketQuote {
        valid: true,
        amount,
        price,
        shares,
        payout,
        error: None,
    }
}

/// Where a placed trade is in its lifecycle — upstream's `status` triple, plus
/// the refusal the invalid arm reads as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarketStatus {
    /// Nothing in flight.
    #[default]
    Idle,
    /// The trade is being placed.
    Placing,
    /// It filled.
    Filled,
}

// ---- The component ---------------------------------------------------------

/// A view-held mode callback (erased on build).
type OnMode<State> = Rc<dyn Fn(&mut State, MarketMode)>;

/// A view-held string callback (erased on build).
type OnString<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held amount callback (erased on build).
type OnAmount<State> = Rc<dyn Fn(&mut State, f64)>;

/// What the ticket renders from, beyond its outcomes.
#[derive(Clone, Debug, PartialEq)]
struct MarketConfig {
    mode: MarketMode,
    outcome_id: Option<String>,
    amount: f64,
    balance: f64,
    min_trade: f64,
    quick_amounts: Vec<f64>,
    order_type_label: String,
    status: MarketStatus,
    authenticated: bool,
}

/// A declarative beUI prediction-market ticket. See [`prediction_market`].
pub struct PredictionMarketView<State: 'static> {
    outcomes: Vec<MarketOutcome>,
    config: MarketConfig,
    on_mode_change: OnMode<State>,
    on_outcome_change: OnString<State>,
    on_amount_change: OnAmount<State>,
    on_trade: Rc<dyn Fn(&mut State)>,
    on_sign_in: Rc<dyn Fn(&mut State)>,
}

/// Build a prediction-market ticket over `outcomes`.
pub fn prediction_market<State: 'static>(
    outcomes: Vec<MarketOutcome>,
) -> PredictionMarketView<State> {
    PredictionMarketView {
        outcomes,
        config: MarketConfig {
            mode: MarketMode::default(),
            outcome_id: None,
            amount: 0.0,
            balance: MARKET_DEFAULT_BALANCE,
            min_trade: MARKET_MIN_TRADE,
            quick_amounts: MARKET_QUICK_AMOUNTS.to_vec(),
            order_type_label: "Market".to_string(),
            status: MarketStatus::default(),
            authenticated: true,
        },
        on_mode_change: Rc::new(|_, _| {}),
        on_outcome_change: Rc::new(|_, _| {}),
        on_amount_change: Rc::new(|_, _| {}),
        on_trade: Rc::new(|_| {}),
        on_sign_in: Rc::new(|_| {}),
    }
}

impl<State: 'static> PredictionMarketView<State> {
    /// Set which side of the market the ticket is on.
    pub fn mode(mut self, mode: MarketMode) -> Self {
        self.config.mode = mode;
        self
    }

    /// Take the selected outcome over.
    pub fn outcome_id(mut self, id: impl Into<String>) -> Self {
        self.config.outcome_id = Some(id.into());
        self
    }

    /// Set the amount being staked.
    pub fn amount(mut self, amount: f64) -> Self {
        self.config.amount = if amount.is_finite() && amount > 0.0 {
            amount
        } else {
            0.0
        };
        self
    }

    /// Set the wallet's balance (`balance`).
    pub fn balance(mut self, balance: f64) -> Self {
        self.config.balance = balance;
        self
    }

    /// Set the least a buy may be (`minTrade`).
    pub fn min_trade(mut self, min_trade: f64) -> Self {
        self.config.min_trade = min_trade;
        self
    }

    /// Set the quick-add chips (`quickAmounts`).
    pub fn quick_amounts(mut self, amounts: Vec<f64>) -> Self {
        self.config.quick_amounts = amounts;
        self
    }

    /// Set the order-type label (`orderTypeLabel`).
    pub fn order_type_label(mut self, label: impl Into<String>) -> Self {
        self.config.order_type_label = label.into();
        self
    }

    /// Set the trade's lifecycle state (`status`) — the app owns it, because
    /// the fill is the app's own asynchronous work.
    pub fn status(mut self, status: MarketStatus) -> Self {
        self.config.status = status;
        self
    }

    /// Set whether the app has a signed-in user (`authenticated`); when it does
    /// not, the ticket shows a single `Connect` action instead of its footer.
    pub fn authenticated(mut self, authenticated: bool) -> Self {
        self.config.authenticated = authenticated;
        self
    }

    /// Set the mode callback.
    pub fn on_mode_change<F: Fn(&mut State, MarketMode) + 'static>(mut self, f: F) -> Self {
        self.on_mode_change = Rc::new(f);
        self
    }

    /// Set the outcome callback.
    pub fn on_outcome_change<F: Fn(&mut State, String) + 'static>(mut self, f: F) -> Self {
        self.on_outcome_change = Rc::new(f);
        self
    }

    /// Set the callback the chips and `Max` report a new amount through.
    pub fn on_amount_change<F: Fn(&mut State, f64) + 'static>(mut self, f: F) -> Self {
        self.on_amount_change = Rc::new(f);
        self
    }

    /// Set the callback a valid trade reports through.
    pub fn on_trade<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_trade = Rc::new(f);
        self
    }

    /// Set the callback `Connect` reports through (`onSignIn`).
    pub fn on_sign_in<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_sign_in = Rc::new(f);
        self
    }

    /// The payout ticker's own view — a [`crate::components::number`] in roll
    /// mode, timed like upstream's `NumberTicker`.
    fn ticker_view(&self) -> AnyView<State> {
        let outcome = self
            .outcomes
            .iter()
            .find(|o| Some(&o.id) == self.config.outcome_id.as_ref())
            .or_else(|| self.outcomes.first());
        let payout = outcome.map_or(0.0, |o| {
            market_quote(
                self.config.mode,
                self.config.amount,
                o,
                self.config.balance,
                self.config.min_trade,
            )
            .payout
        });
        any(number::<State>(payout)
            .mode(NumberMode::Roll)
            .group(true)
            .prefix("$")
            .duration(MARKET_TICKER)
            // `stagger={0}`: the columns move together, not in cascade.
            .stagger(Duration::ZERO)
            .size(MARKET_TICKER_SIZE))
    }
}

/// `text-4xl` — the payout ticker's type size, in logical px.
pub const MARKET_TICKER_SIZE: f64 = 36.0;

/// Which affordance a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// One of the two mode tabs.
    Mode(usize),
    /// The inert order-type affordance.
    OrderType,
    /// One outcome pill.
    Outcome(usize),
    /// One quick-add chip; the last slot is `Max`.
    Chip(usize),
    /// The action button.
    Action,
}

/// One retained outcome: its shaped runs and its own id.
struct OutcomeCell {
    id: String,
    /// The pill's own text — the outcome's label and its price together.
    label: LabelRun,
    /// The outcome's declared label on its own, which is what the quote and the
    /// tone are derived from.
    name: String,
    tone: MarketOutcomeTone,
    rect: Rect,
}

/// The retained widget for a [`PredictionMarketView`].
pub struct PredictionMarketWidget {
    outcomes: Vec<OutcomeCell>,
    /// Each outcome's price, kept beside the painted cells because the quote
    /// needs the whole value and a cell keeps only what it draws.
    prices: Vec<f64>,
    /// Each outcome's holding, for the same reason.
    positions: Vec<f64>,
    config: MarketConfig,
    /// The index of the selected outcome.
    selected: usize,
    modes: [LabelRun; 2],
    order_type: LabelRun,
    amount_label: LabelRun,
    /// The amount's shaped cells, one per character, so entry rolls per digit.
    amount_cells: Vec<LabelRun>,
    /// The cells the previous amount had, still leaving.
    leaving_cells: Vec<LabelRun>,
    chips: Vec<LabelRun>,
    payout_label: LabelRun,
    price_line: LabelRun,
    action: LabelRun,
    /// The payout ticker, hosted as a child pod.
    ticker: ChildPod,
    /// The mode underline's slide: `0.0` on Buy, `1.0` on Sell.
    underline: Lane,
    /// The frame the current digit swap started on.
    digits_since: Option<FrameTime>,
    /// The frame the refusal shake started on, or `None` while it is not
    /// shaking.
    shake_since: Option<FrameTime>,
    /// Set by the event pass that refused a trade: the shake needs a clock, and
    /// only `paint` has one.
    shake_pending: bool,
    /// Each chip's press shrink, plus the action button's in the last slot.
    presses: Vec<Lane>,
    /// The quote as of the last layout.
    quote: MarketQuote,
    /// The boxes the last layout resolved, in the widget's own space.
    mode_rects: [Rect; 2],
    order_type_rect: Rect,
    chip_rects: Vec<Rect>,
    amount_card: Rect,
    action_rect: Rect,
    ticker_rect: Rect,
    /// The affordance a `Down` armed.
    armed: Option<Target>,
    on_mode_change: ErasedArgCallback<MarketMode>,
    on_outcome_change: ErasedArgCallback<String>,
    on_amount_change: ErasedArgCallback<f64>,
    on_trade: ErasedCallback,
    on_sign_in: ErasedCallback,
}

impl PredictionMarketWidget {
    /// The quote as of the last layout.
    pub fn quote(&self) -> &MarketQuote {
        &self.quote
    }

    /// The index of the selected outcome.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether the ticket is mid-refusal.
    pub fn is_shaking(&self) -> bool {
        self.shake_since.is_some() || self.shake_pending
    }

    /// How far the ticket is displaced by its refusal shake at `now`.
    pub fn shake_offset(&self, now: FrameTime) -> f64 {
        let Some(started) = self.shake_since else {
            return 0.0;
        };
        let elapsed = now.saturating_sub(started);
        if elapsed >= MARKET_SHAKE {
            return 0.0;
        }
        keyframes_at(
            &MARKET_SHAKE_FRAMES,
            elapsed.as_secs_f64() / MARKET_SHAKE.as_secs_f64(),
        )
    }

    /// How many chips the tray shows: the declared quick amounts plus `Max`.
    fn chip_count(&self) -> usize {
        self.config.quick_amounts.len() + 1
    }

    /// The affordance `position` lands on, if any.
    fn target_at(&self, position: Point) -> Option<Target> {
        if let Some(index) = self.mode_rects.iter().position(|r| r.contains(position)) {
            return Some(Target::Mode(index));
        }
        if self.order_type_rect.contains(position) {
            return Some(Target::OrderType);
        }
        if let Some(index) = self
            .outcomes
            .iter()
            .position(|cell| cell.rect.contains(position))
        {
            return Some(Target::Outcome(index));
        }
        if let Some(index) = self.chip_rects.iter().position(|r| r.contains(position)) {
            return Some(Target::Chip(index));
        }
        if self.action_rect.contains(position) {
            return Some(Target::Action);
        }
        None
    }

    /// Act on a released press.
    fn fire(&mut self, ctx: &mut EventCtx, target: Target) {
        // Every interaction but the action itself clears a finished trade, so
        // a filled ticket returns to idle the moment it is touched — upstream's
        // own `setOrderValue`.
        match target {
            Target::Mode(index) => {
                let mode = MarketMode::ALL[index.min(1)];
                if mode != self.config.mode {
                    (self.on_mode_change)(ctx, mode);
                    // "A mode switch clears the amount" is upstream's own
                    // `setOrderValue({ mode, amount: "" })`.
                    (self.on_amount_change)(ctx, 0.0);
                }
            }
            // Upstream's order-type affordance opens nothing; a port that
            // invented a menu for it would be inventing a component.
            Target::OrderType => {}
            Target::Outcome(index) => {
                if let Some(cell) = self.outcomes.get(index) {
                    let id = cell.id.clone();
                    if self.config.outcome_id.is_none() {
                        self.selected = index;
                    }
                    (self.on_outcome_change)(ctx, id);
                }
            }
            Target::Chip(index) => {
                if index < self.config.quick_amounts.len() {
                    let next = self.config.amount + self.config.quick_amounts[index];
                    (self.on_amount_change)(ctx, next);
                } else {
                    let max = self.max_amount();
                    (self.on_amount_change)(ctx, max);
                }
            }
            Target::Action => {
                if !self.config.authenticated {
                    (self.on_sign_in)(ctx);
                } else if self.quote.valid {
                    (self.on_trade)(ctx);
                } else {
                    // A refused trade shakes rather than reporting anything.
                    self.shake_pending = true;
                }
            }
        }
        ctx.request_redraw();
    }

    /// What `Max` sets: the whole balance on a buy (floored, as upstream's
    /// `Math.floor` does), the whole holding on a sell.
    fn max_amount(&self) -> f64 {
        match self.config.mode {
            MarketMode::Buy => self.config.balance.floor().max(0.0),
            MarketMode::Sell => self
                .outcomes
                .get(self.selected)
                .map_or(0.0, |_| self.selected_position()),
        }
    }

    /// The wallet's holding of the selected outcome.
    fn selected_position(&self) -> f64 {
        self.positions.get(self.selected).copied().unwrap_or(0.0)
    }

    /// The action button's own label, given the trade's state.
    fn action_label(&self) -> String {
        if !self.config.authenticated {
            return "Connect".to_string();
        }
        match self.config.status {
            MarketStatus::Placing => "Trading".to_string(),
            MarketStatus::Filled => "Trade filled".to_string(),
            MarketStatus::Idle => match &self.quote.error {
                Some(error) => error.clone(),
                None => "Trade".to_string(),
            },
        }
    }
}

impl<State: 'static> View<State> for PredictionMarketView<State> {
    type Element = PredictionMarketWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PredictionMarketWidget {
        let selected = self.selected_index();
        let outcome = self.outcomes.get(selected);
        let quote = outcome.map_or_else(empty_quote, |o| {
            market_quote(
                self.config.mode,
                self.config.amount,
                o,
                self.config.balance,
                self.config.min_trade,
            )
        });
        let mut widget = PredictionMarketWidget {
            outcomes: outcome_cells(&self.outcomes),
            prices: self.outcomes.iter().map(|o| o.price).collect(),
            positions: self.outcomes.iter().map(|o| o.position).collect(),
            modes: [
                LabelRun::new(MarketMode::Buy.label()),
                LabelRun::new(MarketMode::Sell.label()),
            ],
            order_type: LabelRun::new(self.config.order_type_label.clone()),
            amount_label: LabelRun::new(self.config.mode.amount_label()),
            amount_cells: amount_cells(self.config.amount),
            leaving_cells: Vec::new(),
            chips: chip_labels(&self.config),
            payout_label: LabelRun::new(self.config.mode.payout_label()),
            price_line: LabelRun::new(format!("Avg. Price {}", format_cents(quote.price))),
            action: LabelRun::new(String::new()),
            ticker: build_child(&self.ticker_view(), ctx),
            underline: Lane::at_rest(
                Ramp::spring(SPRING_SWAP),
                if self.config.mode == MarketMode::Sell {
                    1.0
                } else {
                    0.0
                },
            ),
            digits_since: None,
            shake_since: None,
            shake_pending: false,
            presses: Vec::new(),
            quote,
            selected,
            config: self.config.clone(),
            mode_rects: [Rect::ZERO; 2],
            order_type_rect: Rect::ZERO,
            chip_rects: Vec::new(),
            amount_card: Rect::ZERO,
            action_rect: Rect::ZERO,
            ticker_rect: Rect::ZERO,
            armed: None,
            on_mode_change: erase_callback_arg(&self.on_mode_change),
            on_outcome_change: erase_callback_arg(&self.on_outcome_change),
            on_amount_change: erase_callback_arg(&self.on_amount_change),
            on_trade: erase_callback(&self.on_trade),
            on_sign_in: erase_callback(&self.on_sign_in),
        };
        widget.presses = (0..widget.chip_count() + 1)
            .map(|_| Lane::at_rest(Ramp::eased(MARKET_DIGIT_TRANSITION, EASE_OUT), 0.0))
            .collect();
        widget.action.set_content(widget.action_label());
        widget
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PredictionMarketWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = rebuild_child(
            &prev.ticker_view(),
            &self.ticker_view(),
            &mut element.ticker,
            ctx,
        );
        if prev.outcomes != self.outcomes {
            element.outcomes = outcome_cells(&self.outcomes);
            element.prices = self.outcomes.iter().map(|o| o.price).collect();
            element.positions = self.outcomes.iter().map(|o| o.position).collect();
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.config != self.config {
            if element.config.amount != self.config.amount {
                // The characters that are going out keep playing; the new ones
                // roll in behind them, which is `mode="popLayout"`.
                element.leaving_cells = std::mem::take(&mut element.amount_cells);
                element.amount_cells = amount_cells(self.config.amount);
                element.digits_since = None;
            }
            if element.config.mode != self.config.mode {
                element
                    .underline
                    .retarget(if self.config.mode == MarketMode::Sell {
                        1.0
                    } else {
                        0.0
                    });
                element
                    .amount_label
                    .set_content(self.config.mode.amount_label());
                element
                    .payout_label
                    .set_content(self.config.mode.payout_label());
            }
            element
                .order_type
                .set_content(self.config.order_type_label.clone());
            element.chips = chip_labels(&self.config);
            element.config = self.config.clone();
            if element.presses.len() != element.chip_count() + 1 {
                element.presses = (0..element.chip_count() + 1)
                    .map(|_| Lane::at_rest(Ramp::eased(MARKET_DIGIT_TRANSITION, EASE_OUT), 0.0))
                    .collect();
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        element.selected = self.selected_index();
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_mode_change = erase_callback_arg(&self.on_mode_change);
        element.on_outcome_change = erase_callback_arg(&self.on_outcome_change);
        element.on_amount_change = erase_callback_arg(&self.on_amount_change);
        element.on_trade = erase_callback(&self.on_trade);
        element.on_sign_in = erase_callback(&self.on_sign_in);
        flags
    }

    fn teardown(&self, element: &mut PredictionMarketWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.ticker_view(), &mut element.ticker, ctx);
    }
}

impl<State: 'static> PredictionMarketView<State> {
    /// The index of the selected outcome — the declared id's, or the first.
    fn selected_index(&self) -> usize {
        let Some(id) = self.config.outcome_id.as_ref() else {
            return 0;
        };
        self.outcomes
            .iter()
            .position(|outcome| &outcome.id == id)
            .unwrap_or(0)
    }
}

/// The quote a ticket with no outcomes reports.
fn empty_quote() -> MarketQuote {
    MarketQuote {
        valid: false,
        amount: 0.0,
        price: 0.01,
        shares: 0.0,
        payout: 0.0,
        error: Some("Enter an amount".to_string()),
    }
}

/// Build the retained cells for a declared outcome list.
fn outcome_cells(outcomes: &[MarketOutcome]) -> Vec<OutcomeCell> {
    outcomes
        .iter()
        .map(|outcome| OutcomeCell {
            id: outcome.id.clone(),
            label: LabelRun::new(format!("{} {}", outcome.label, format_cents(outcome.price))),
            name: outcome.label.clone(),
            tone: outcome.tone(),
            rect: Rect::ZERO,
        })
        .collect()
}

/// One shaped run per character of the amount, `0` when it is empty.
fn amount_cells(amount: f64) -> Vec<LabelRun> {
    let text = if amount > 0.0 {
        if amount.fract() == 0.0 {
            format!("{}", amount as i64)
        } else {
            format!("{amount:.2}")
        }
    } else {
        "0".to_string()
    };
    text.chars()
        .map(|ch| LabelRun::new(ch.to_string()))
        .collect()
}

/// The chip tray's labels: `+$10`-style quick adds on a buy, bare counts on a
/// sell, then `Max`.
fn chip_labels(config: &MarketConfig) -> Vec<LabelRun> {
    let mut labels: Vec<LabelRun> = config
        .quick_amounts
        .iter()
        .map(|amount| {
            LabelRun::new(match config.mode {
                MarketMode::Buy => format!("+{}", format_compact_currency(*amount)),
                MarketMode::Sell => format!("+{}", format_amount_plain(*amount)),
            })
        })
        .collect();
    labels.push(LabelRun::new("Max"));
    labels
}

/// A plain count, without a currency mark or trailing zeros.
fn format_amount_plain(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// One label style at `size`, in the theme's `label_large` family.
fn market_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    themed_style(
        crate::text::label_style(size),
        ThemeTextType::LabelLarge,
        theme,
    )
}

impl Widget for PredictionMarketWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        // Every style is resolved before the first `layout` call: the shaper
        // takes `ctx` mutably, and the theme read borrows it.
        let body = market_style(theme, style::TEXT_SM);
        let big = market_style(theme, MARKET_MODE_SIZE);
        let outcome_style = market_style(theme, style::TEXT_BASE);
        let digits = self
            .amount_cells
            .iter()
            .filter(|cell| cell.content().chars().all(|c| c.is_ascii_digit()))
            .count();
        let amount_style = market_style(theme, market_amount_size(digits));

        // The quote, recomputed from the current order.
        self.quote = match self.outcomes.get(self.selected) {
            Some(_) => market_quote(
                self.config.mode,
                self.config.amount,
                &self.outcome_at(self.selected),
                self.config.balance,
                self.config.min_trade,
            ),
            None => empty_quote(),
        };
        self.price_line
            .set_content(format!("Avg. Price {}", format_cents(self.quote.price)));
        let label = self.action_label();
        self.action.set_content(label);

        self.order_type.layout(ctx, &big);
        self.amount_label.layout(ctx, &big);
        self.payout_label.layout(ctx, &big);
        self.price_line.layout(ctx, &body);
        self.action.layout(ctx, &body);
        for run in &mut self.modes {
            run.layout(ctx, &big);
        }
        for cell in self
            .amount_cells
            .iter_mut()
            .chain(self.leaving_cells.iter_mut())
        {
            cell.layout(ctx, &amount_style);
        }
        for chip in &mut self.chips {
            chip.layout(ctx, &body);
        }
        for cell in &mut self.outcomes {
            cell.label.layout(ctx, &outcome_style);
        }

        let width = bc.constrain(Size::new(MARKET_WIDTH, 0.0)).width;
        let inner = (width - MARKET_PADDING * 2.0).max(0.0);

        // The header: two mode tabs on the left, the order type on the right.
        let mut x = MARKET_PADDING;
        let mode_height = self.modes[0].size().height + MARKET_MODE_GAP;
        for index in 0..2 {
            let size = self.modes[index].size();
            self.mode_rects[index] = Rect::from_origin_size(
                Point::new(x, MARKET_PADDING),
                Size::new(size.width, mode_height),
            );
            x += size.width + MARKET_MODE_SPACING;
        }
        let order = self.order_type.size();
        self.order_type_rect = Rect::from_origin_size(
            Point::new(
                width - MARKET_PADDING - order.width - style::ICON_SIZE_LG,
                MARKET_PADDING,
            ),
            Size::new(order.width + style::ICON_SIZE_LG, mode_height),
        );
        let header_bottom = MARKET_PADDING + mode_height + MARKET_UNDERLINE_HEIGHT;

        // The outcome tray.
        let mut y = header_bottom + MARKET_BODY_PADDING;
        let tray_inner = (width - MARKET_BODY_PADDING * 2.0 - MARKET_OUTCOME_GAP * 1.5).max(0.0);
        let count = self.outcomes.len().max(1);
        let pill = ((tray_inner - MARKET_OUTCOME_GAP * (count - 1) as f64) / count as f64).max(0.0);
        for (index, cell) in self.outcomes.iter_mut().enumerate() {
            cell.rect = Rect::from_origin_size(
                Point::new(
                    MARKET_BODY_PADDING
                        + MARKET_OUTCOME_GAP * 0.75
                        + index as f64 * (pill + MARKET_OUTCOME_GAP),
                    y + MARKET_OUTCOME_GAP * 0.75,
                ),
                Size::new(pill, MARKET_OUTCOME_HEIGHT),
            );
        }
        y += MARKET_OUTCOME_HEIGHT + MARKET_OUTCOME_GAP * 1.5 + style::GAP_MD;

        // The amount card: the label, the rolling amount, then the chip tray.
        let amount_height = self
            .amount_cells
            .first()
            .map_or(MARKET_AMOUNT_MIN_HEIGHT, |cell| cell.size().height);
        let chips_y = y
            + MARKET_AMOUNT_PADDING
            + self.amount_label.size().height
            + style::GAP_MD
            + amount_height.max(MARKET_AMOUNT_MIN_HEIGHT - 24.0)
            + MARKET_AMOUNT_PADDING * 2.0;
        let mut chip_x = MARKET_BODY_PADDING + MARKET_AMOUNT_PADDING;
        self.chip_rects = self
            .chips
            .iter()
            .map(|chip| {
                let chip_width = chip.size().width + MARKET_CHIP_PADDING_X * 2.0;
                let rect = Rect::from_origin_size(
                    Point::new(chip_x, chips_y),
                    Size::new(chip_width, MARKET_CHIP_HEIGHT),
                );
                chip_x += chip_width + MARKET_CHIP_GAP;
                rect
            })
            .collect();
        let card_bottom = chips_y + MARKET_CHIP_HEIGHT + MARKET_AMOUNT_PADDING;
        self.amount_card = Rect::new(
            MARKET_BODY_PADDING,
            y,
            width - MARKET_BODY_PADDING,
            card_bottom,
        );

        // The footer, or the bare `Connect` action when nobody is signed in.
        y = card_bottom + MARKET_BODY_PADDING;
        if self.config.authenticated {
            let payout = self.payout_label.size();
            self.ticker_rect = Rect::from_origin_size(
                Point::new(MARKET_PADDING, y + style::GAP_MD),
                Size::new(inner, MARKET_TICKER_SIZE * 1.4),
            );
            self.ticker.layout_child(
                ctx,
                &BoxConstraints::new(Size::ZERO, self.ticker_rect.size()),
            );
            let ticker = self.ticker.size();
            self.ticker.set_origin(Point::new(
                self.ticker_rect.x1 - ticker.width,
                self.ticker_rect.y0,
            ));
            y += style::GAP_MD
                + payout.height.max(ticker.height)
                + self.price_line.size().height
                + style::GAP_MD * 2.0;
        } else {
            self.ticker
                .layout_child(ctx, &BoxConstraints::tight(Size::ZERO));
            self.ticker_rect = Rect::ZERO;
        }
        self.action_rect = Rect::from_origin_size(
            Point::new(MARKET_PADDING, y),
            Size::new(inner, MARKET_ACTION_HEIGHT),
        );

        bc.constrain(Size::new(width, self.action_rect.y1 + MARKET_PADDING))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let tokens = crate::BeuiTokens::resolve(theme);
        let (accent, on_accent, card, danger) = match theme {
            Some(t) => (
                t.scheme().primary,
                t.scheme().on_primary,
                t.scheme().surface_container,
                t.scheme().error,
            ),
            None => (
                crate::BEUI_LIGHT.primary,
                crate::BEUI_LIGHT.primary_foreground,
                crate::BEUI_LIGHT.muted,
                crate::BEUI_LIGHT.destructive,
            ),
        };
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        if std::mem::take(&mut self.shake_pending) && !reduce {
            self.shake_since = Some(now);
        }
        if let Some(started) = self.shake_since {
            if now.saturating_sub(started) >= MARKET_SHAKE {
                self.shake_since = None;
            } else {
                ctx.request_frame();
            }
        }
        if reduce {
            self.underline.snap();
            for lane in &mut self.presses {
                lane.snap();
            }
        } else {
            let mut animating = self.underline.advance(now);
            for lane in &mut self.presses {
                animating |= lane.advance(now);
            }
            if animating {
                ctx.request_frame();
            }
        }
        let digits_started = *self.digits_since.get_or_insert(now);
        let digits_elapsed = if reduce {
            MARKET_DIGIT_TRANSITION
        } else {
            now.saturating_sub(digits_started)
        };
        if digits_elapsed < MARKET_DIGIT_TRANSITION {
            ctx.request_frame();
        } else if !self.leaving_cells.is_empty() {
            self.leaving_cells.clear();
        }

        // `animate(amountRef, { x: [...] })` shakes the whole ticket, so the
        // displacement is a transform over everything it paints rather than an
        // offset threaded through each box.
        let shake = self.shake_offset(now);
        if shake != 0.0 {
            scene.push_transform(Affine::translate((shake, 0.0)));
        }

        let radius = style::resolve_radius(MARKET_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, chrome.surface);
        paint_panel_hairline(scene, origin, size, radius, chrome.border);

        self.paint_header(scene, origin, size, chrome);
        self.paint_outcomes(scene, origin, chrome, tokens.success, danger);
        self.paint_amount(scene, origin, chrome, card, digits_elapsed, now);
        if self.config.authenticated {
            self.paint_footer(scene, origin, size, chrome, tokens.success);
            self.ticker.paint_child(ctx, scene);
        }
        self.paint_action(scene, origin, chrome, accent, on_accent);
        if shake != 0.0 {
            scene.pop_transform();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if route_event_single(&mut self.ticker, ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                self.fire(ctx, Target::Action);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let action = self.action.content().to_string();
        let placing = self.config.status == MarketStatus::Placing;
        ctx.push_container(
            Role::Form,
            |node| {
                node.set_label("Trade ticket");
            },
            |ctx| {
                for (index, mode) in MarketMode::ALL.into_iter().enumerate() {
                    let selected = mode == self.config.mode;
                    let label = self.modes[index].content().to_string();
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(label.as_str());
                        node.set_selected(selected);
                        node.add_action(Action::Click);
                    });
                }
                for (index, cell) in self.outcomes.iter().enumerate() {
                    let label = cell.label.content().to_string();
                    let selected = index == self.selected;
                    ctx.push_node(Role::Tab, |node| {
                        node.set_label(label.as_str());
                        node.set_selected(selected);
                        node.add_action(Action::Click);
                    });
                }
                let amount = format!(
                    "{}, {}",
                    self.amount_label.content(),
                    self.amount_cells
                        .iter()
                        .map(|cell| cell.content())
                        .collect::<String>()
                );
                ctx.push_node(Role::Label, |node| {
                    node.set_label(amount.as_str());
                });
                for chip in &self.chips {
                    let label = chip.content().to_string();
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(label.as_str());
                        if placing {
                            node.set_disabled();
                        } else {
                            node.add_action(Action::Click);
                        }
                    });
                }
                if self.config.authenticated {
                    // The payout is the ticker's own node, forwarded rather
                    // than restated.
                    self.ticker.semantics_child(ctx);
                }
                ctx.push_node(Role::Button, |node| {
                    node.set_label(action.as_str());
                    node.add_action(Action::Click);
                });
            },
        );
    }

    visit_children!(ticker);
}

impl PredictionMarketWidget {
    /// The outcome at `index`, rebuilt from the retained cell and the declared
    /// position — the quote needs the whole value, and the cell keeps only what
    /// it paints.
    fn outcome_at(&self, index: usize) -> MarketOutcome {
        let cell = &self.outcomes[index];
        MarketOutcome {
            id: cell.id.clone(),
            label: cell.name.clone(),
            price: self.prices.get(index).copied().unwrap_or(0.5),
            position: self.positions.get(index).copied().unwrap_or(0.0),
        }
    }

    /// The header: two mode tabs, the sliding underline and the order type.
    fn paint_header(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        chrome: PanelChrome,
    ) {
        for index in 0..2 {
            let rect = self.mode_rects[index];
            let selected = MarketMode::ALL[index] == self.config.mode;
            self.modes[index].paint(
                origin + rect.origin().to_vec2(),
                if selected { chrome.ink } else { chrome.dim_ink },
                scene,
            );
        }
        // The underline slides between the two tabs rather than jumping.
        let t = self.underline.value().clamp(0.0, 1.0);
        let from = self.mode_rects[0];
        let to = self.mode_rects[1];
        let x = from.x0 + (to.x0 - from.x0) * t;
        let w = from.width() + (to.width() - from.width()) * t;
        scene.fill_rect(
            origin + Vec2::new(x, from.y1),
            Size::new(w, MARKET_UNDERLINE_HEIGHT),
            chrome.ink,
        );

        let order = self.order_type.size();
        self.order_type.paint(
            origin + self.order_type_rect.origin().to_vec2(),
            chrome.ink,
            scene,
        );
        let _ = order;
        scene.fill_rect(
            origin + Vec2::new(0.0, self.mode_rects[0].y1 + MARKET_UNDERLINE_HEIGHT),
            Size::new(size.width, style::BORDER_WIDTH),
            chrome.border,
        );
    }

    /// The outcome tray: one tinted pill per outcome.
    fn paint_outcomes(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        success: Color,
        danger: Color,
    ) {
        for (index, cell) in self.outcomes.iter().enumerate() {
            let hue = match cell.tone {
                MarketOutcomeTone::Yes => success,
                MarketOutcomeTone::No => danger,
            };
            let selected = index == self.selected;
            let at = origin + cell.rect.origin().to_vec2();
            if selected {
                scene.fill_rounded_rect(
                    at,
                    cell.rect.size(),
                    MARKET_OUTCOME_RADIUS,
                    style::with_alpha(hue, MARKET_OUTCOME_FILL_ALPHA),
                );
            }
            let label = cell.label.size();
            cell.label.paint(
                at + Vec2::new(
                    (cell.rect.width() - label.width) / 2.0,
                    (cell.rect.height() - label.height) / 2.0,
                ),
                if selected {
                    hue
                } else {
                    style::scale_alpha(hue, 0.55)
                },
                scene,
            );
            let _ = chrome;
        }
    }

    /// The amount card: its label, its rolling characters and the chip tray.
    fn paint_amount(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        card: Color,
        elapsed: Duration,
        _now: FrameTime,
    ) {
        let at = origin + self.amount_card.origin().to_vec2();
        scene.fill_rounded_rect(at, self.amount_card.size(), MARKET_RADIUS, card);

        let label = self.amount_label.size();
        self.amount_label.paint(
            Point::new(
                at.x + (self.amount_card.width() - label.width) / 2.0,
                at.y + MARKET_AMOUNT_PADDING,
            ),
            chrome.ink,
            scene,
        );

        // The characters: the leaving ones rising out, the arriving ones
        // rising in, both on `DIGIT_TRANSITION`.
        let progress = Ramp::eased(MARKET_DIGIT_TRANSITION, EASE_OUT).progress_clamped(elapsed);
        let y = at.y + MARKET_AMOUNT_PADDING + label.height + style::GAP_MD;
        let centre_x = at.x + self.amount_card.width() / 2.0;
        paint_amount_row(
            scene,
            centre_x,
            y + MARKET_DIGIT_EXIT_TRAVEL * progress,
            &self.leaving_cells,
            style::with_alpha(chrome.ink, (1.0 - progress) as f32),
        );
        paint_amount_row(
            scene,
            centre_x,
            y + MARKET_DIGIT_TRAVEL * (1.0 - progress),
            &self.amount_cells,
            style::with_alpha(chrome.ink, progress as f32),
        );

        // The chip tray.
        let placing = self.config.status == MarketStatus::Placing;
        for (index, rect) in self.chip_rects.iter().enumerate() {
            let scale = press_scale(
                MARKET_CHIP_PRESS_SCALE,
                self.presses.get(index).map_or(0.0, Lane::value),
            );
            let box_size = Size::new(rect.width() * scale, rect.height() * scale);
            let chip_at = origin
                + Vec2::new(
                    rect.x0 + (rect.width() - box_size.width) / 2.0,
                    rect.y0 + (rect.height() - box_size.height) / 2.0,
                );
            scene.fill_rounded_rect(chip_at, box_size, style::RADIUS_XL, chrome.surface);
            let text = self.chips[index].size();
            self.chips[index].paint(
                chip_at
                    + Vec2::new(
                        (box_size.width - text.width) / 2.0,
                        (box_size.height - text.height) / 2.0,
                    ),
                style::disabled_tint(chrome.ink, placing, MARKET_DISABLED_OPACITY),
                scene,
            );
        }
    }

    /// The footer: the payout label, the average price and the ticker's slot.
    fn paint_footer(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        chrome: PanelChrome,
        success: Color,
    ) {
        scene.fill_rect(
            origin + Vec2::new(0.0, self.ticker_rect.y0 - style::GAP_MD),
            Size::new(size.width, style::BORDER_WIDTH),
            chrome.border,
        );
        let payout = self.payout_label.size();
        self.payout_label.paint(
            origin + Vec2::new(MARKET_PADDING, self.ticker_rect.y0),
            chrome.ink,
            scene,
        );
        self.price_line.paint(
            origin
                + Vec2::new(
                    MARKET_PADDING,
                    self.ticker_rect.y0 + payout.height + style::GAP_SM,
                ),
            chrome.dim_ink,
            scene,
        );
        let _ = success;
    }

    /// The action button: its fill, its press shrink and its label.
    fn paint_action(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        accent: Color,
        on_accent: Color,
    ) {
        let index = self.presses.len().saturating_sub(1);
        let scale = press_scale(
            MARKET_ACTION_PRESS_SCALE,
            self.presses.get(index).map_or(0.0, Lane::value),
        );
        let rect = self.action_rect;
        let box_size = Size::new(rect.width() * scale, rect.height() * scale);
        let at = origin
            + Vec2::new(
                rect.x0 + (rect.width() - box_size.width) / 2.0,
                rect.y0 + (rect.height() - box_size.height) / 2.0,
            );
        scene.fill_rounded_rect(at, box_size, style::RADIUS_2XL, accent);
        let label = self.action.size();
        self.action.paint(
            at + Vec2::new(
                (box_size.width - label.width) / 2.0,
                (box_size.height - label.height) / 2.0,
            ),
            on_accent,
            scene,
        );
        let _ = chrome;
    }

    /// The `Widget::event` pointer arm.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        if !inside(p.position, size) && p.phase != PointerPhase::Up {
            return EventResult::Ignored;
        }
        let over = self.target_at(p.position);
        // Every affordance but the action itself is inert while a trade is in
        // flight — upstream's `disabled={status === "placing"}`.
        let frozen = self.config.status == MarketStatus::Placing;
        match p.phase {
            PointerPhase::Move => {
                if over.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                ctx.request_focus();
                let Some(target) = over else {
                    return EventResult::Ignored;
                };
                if frozen && target != Target::Action {
                    return EventResult::Ignored;
                }
                ctx.capture_pointer();
                self.armed = Some(target);
                match target {
                    Target::Chip(index) => {
                        if let Some(lane) = self.presses.get_mut(index) {
                            lane.retarget(1.0);
                        }
                    }
                    Target::Action => {
                        let index = self.presses.len().saturating_sub(1);
                        if let Some(lane) = self.presses.get_mut(index) {
                            lane.retarget(1.0);
                        }
                    }
                    _ => {}
                }
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                for lane in &mut self.presses {
                    lane.retarget(0.0);
                }
                ctx.request_redraw();
                if over == Some(armed) {
                    self.fire(ctx, armed);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                if self.armed.take().is_none() {
                    return EventResult::Ignored;
                }
                for lane in &mut self.presses {
                    lane.retarget(0.0);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

/// Paint one row of amount characters, centred on `centre_x`.
fn paint_amount_row(
    scene: &mut dyn PaintScene,
    centre_x: f64,
    y: f64,
    cells: &[LabelRun],
    ink: Color,
) {
    let width: f64 = cells.iter().map(|cell| cell.size().width).sum();
    let mut x = centre_x - width / 2.0;
    for cell in cells {
        cell.paint(Point::new(x, y), ink, scene);
        x += cell.size().width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(500.0, 900.0);

    // ---- Formatting ---------------------------------------------------------

    #[test]
    fn a_price_reads_as_cents_and_keeps_a_decimal_only_when_it_has_one() {
        assert_eq!(format_cents(0.09), "9¢");
        assert_eq!(format_cents(0.91), "91¢");
        assert_eq!(format_cents(1.0), "100¢");
        assert_eq!(format_cents(0.0), "0¢");
        assert_eq!(format_cents(0.095), "9.5¢");
    }

    #[test]
    fn currency_groups_its_thousands_and_the_compact_form_drops_whole_cents() {
        assert_eq!(format_currency(0.0, 2), "$0.00");
        assert_eq!(format_currency(1_234.5, 2), "$1,234.50");
        assert_eq!(format_currency(1_234_567.0, 0), "$1,234,567");
        assert_eq!(format_currency(-5.0, 2), "-$5.00");
        // Compact: whole dollars from 100 up, and no decimals on a whole value.
        assert_eq!(format_compact_currency(500.0), "$500");
        // Deliberately not a `.5` tie: Rust's formatter rounds ties to even
        // and `Intl.NumberFormat` rounds them away from zero, so a tie would
        // pin a difference this port does not intend to assert either way.
        assert_eq!(format_compact_currency(100.6), "$101");
        assert_eq!(format_compact_currency(10.0), "$10");
        assert_eq!(format_compact_currency(10.5), "$10.50");
    }

    #[test]
    fn the_amount_type_size_steps_down_as_the_number_grows() {
        let sizes: Vec<f64> = [1usize, 5, 6, 8, 10, 14]
            .into_iter()
            .map(market_amount_size)
            .collect();
        for pair in sizes.windows(2) {
            assert!(pair[0] >= pair[1], "sizes grew: {sizes:?}");
        }
        assert_eq!(market_amount_size(1), market_amount_size(5));
        assert!(market_amount_size(6) < market_amount_size(5));
        assert!(market_amount_size(10) < market_amount_size(8));
    }

    // ---- The quote ----------------------------------------------------------

    fn up() -> MarketOutcome {
        market_outcome("up", "Up", 0.09).position(24.0)
    }

    fn down() -> MarketOutcome {
        market_outcome("down", "Down", 0.91).position(16.0)
    }

    #[test]
    fn a_buy_converts_cash_into_shares_at_the_clamped_price() {
        let quote = market_quote(MarketMode::Buy, 90.0, &up(), 500.0, 1.0);
        assert!(quote.valid);
        assert_eq!(quote.price, 0.09);
        assert!((quote.shares - 1_000.0).abs() < 1e-9, "{}", quote.shares);
        assert_eq!(quote.payout, quote.shares, "a buy pays out its shares");
        assert!(quote.error.is_none());
    }

    #[test]
    fn a_sell_converts_shares_into_cash_at_the_same_price() {
        let quote = market_quote(MarketMode::Sell, 10.0, &up(), 500.0, 1.0);
        assert!(quote.valid);
        assert_eq!(quote.shares, 10.0, "a sell moves the shares it names");
        assert!((quote.payout - 0.9).abs() < 1e-9, "{}", quote.payout);
    }

    #[test]
    fn the_price_is_clamped_before_anything_divides_by_it() {
        let free = market_outcome("x", "Yes", 0.0);
        let certain = market_outcome("y", "Yes", 1.0);
        assert_eq!(
            market_quote(MarketMode::Buy, 10.0, &free, 500.0, 1.0).price,
            0.01
        );
        assert_eq!(
            market_quote(MarketMode::Buy, 10.0, &certain, 500.0, 1.0).price,
            0.99
        );
        // ...so the share count stays finite.
        assert!(
            market_quote(MarketMode::Buy, 10.0, &free, 500.0, 1.0)
                .shares
                .is_finite()
        );
    }

    #[test]
    fn the_five_refusal_branches_fire_in_the_upstream_order() {
        // No amount comes first, whatever else is wrong.
        let none = market_quote(MarketMode::Buy, 0.0, &up(), 0.0, 100.0);
        assert!(!none.valid);
        assert_eq!(none.error.as_deref(), Some("Enter an amount"));
        assert_eq!(none.shares, 0.0);
        assert_eq!(none.payout, 0.0);

        // Then the minimum, then the balance.
        let small = market_quote(MarketMode::Buy, 0.5, &up(), 500.0, 1.0);
        assert_eq!(small.error.as_deref(), Some("Minimum $1"));
        let broke = market_quote(MarketMode::Buy, 900.0, &up(), 500.0, 1.0);
        assert_eq!(broke.error.as_deref(), Some("Insufficient balance"));
        // A refused quote still reports its arithmetic.
        assert!(broke.shares > 0.0);

        // A sell is checked against the holding rather than the balance.
        let over = market_quote(MarketMode::Sell, 100.0, &up(), 500.0, 1.0);
        assert_eq!(over.error.as_deref(), Some("Not enough shares"));
        let ok = market_quote(MarketMode::Sell, 20.0, &up(), 0.0, 1.0);
        assert!(ok.valid, "a sell never consults the cash balance");
    }

    #[test]
    fn an_outcome_is_tinted_by_its_own_label() {
        assert_eq!(up().tone(), MarketOutcomeTone::Yes);
        assert_eq!(down().tone(), MarketOutcomeTone::No);
        assert_eq!(MarketOutcomeTone::of("No"), MarketOutcomeTone::No);
        assert_eq!(MarketOutcomeTone::of("DOWN"), MarketOutcomeTone::No);
        assert_eq!(MarketOutcomeTone::of("Maybe"), MarketOutcomeTone::Yes);
    }

    #[test]
    fn each_mode_names_its_own_quantity_and_payout() {
        assert_eq!(MarketMode::Buy.amount_label(), "Amount");
        assert_eq!(MarketMode::Sell.amount_label(), "Shares");
        assert_eq!(MarketMode::Buy.payout_label(), "To win");
        assert_eq!(MarketMode::Sell.payout_label(), "To receive");
    }

    // ---- The refusal shake ---------------------------------------------------

    #[test]
    fn the_shake_starts_and_ends_at_rest_and_swings_both_ways() {
        assert_eq!(MARKET_SHAKE_FRAMES[0], 0.0);
        assert_eq!(*MARKET_SHAKE_FRAMES.last().expect("a frame"), 0.0);
        assert!(MARKET_SHAKE_FRAMES.iter().any(|x| *x < 0.0));
        assert!(MARKET_SHAKE_FRAMES.iter().any(|x| *x > 0.0));
        // ...and the amplitude decays rather than holding.
        let peaks: Vec<f64> = MARKET_SHAKE_FRAMES.iter().map(|x| x.abs()).collect();
        assert!(peaks[1] > peaks[3] && peaks[3] > peaks[5]);
        // Sampled through `keyframes_at`, it lands exactly on its own frames.
        assert_eq!(keyframes_at(&MARKET_SHAKE_FRAMES, 0.0), 0.0);
        assert_eq!(keyframes_at(&MARKET_SHAKE_FRAMES, 1.0), 0.0);
    }

    // ---- The mounted ticket -------------------------------------------------

    #[derive(Default)]
    struct App {
        modes: Vec<MarketMode>,
        outcomes: Vec<String>,
        amounts: Vec<f64>,
        trades: u32,
        sign_ins: u32,
        mode: MarketMode,
        amount: f64,
        outcome: String,
        status: MarketStatus,
        authenticated: bool,
    }

    struct Harness {
        root: RenderRoot<App, frust::StackView<App>>,
        state: App,
        tcx: TextContext,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::themed(light())
        }

        fn themed(theme: Theme) -> Self {
            let mut h = Harness {
                root: RenderRoot::new(),
                state: App {
                    outcome: "up".to_string(),
                    authenticated: true,
                    ..App::default()
                },
                tcx: TextContext::new(),
                clock: 0.0,
            };
            h.root.set_theme(Box::new(theme));
            h.step(0.0);
            h
        }

        fn step(&mut self, ms: f64) -> Recorder {
            self.clock += ms;
            let mut logic = move |s: &mut App| {
                let ticket = prediction_market(vec![up(), down()])
                    .mode(s.mode)
                    .outcome_id(s.outcome.clone())
                    .amount(s.amount)
                    .status(s.status)
                    .authenticated(s.authenticated)
                    .on_mode_change(|s: &mut App, mode| {
                        s.modes.push(mode);
                        s.mode = mode;
                    })
                    .on_outcome_change(|s: &mut App, id| {
                        s.outcomes.push(id.clone());
                        s.outcome = id;
                    })
                    .on_amount_change(|s: &mut App, amount| {
                        s.amounts.push(amount);
                        s.amount = amount;
                    })
                    .on_trade(|s: &mut App| s.trades += 1)
                    .on_sign_in(|s: &mut App| s.sign_ins += 1);
                frust::stack().child(ticket)
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            let now = self.clock;
            self.root.paint(&mut rec, ft_ms(now));
            rec
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        fn click(&mut self, at: Point) {
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.step(0.0);
        }

        /// A point inside the `Sell` tab.
        ///
        /// Resolved from the mode underline the header paints — the only
        /// 2px-tall `fill_rect` on the card — because a tab's own width comes
        /// from its shaped label rather than from a constant.
        fn sell_tab(&mut self) -> Point {
            // Settle the underline first, so it is sitting on a tab rather than
            // between the two.
            let rec = self.step(2_000.0);
            let bar = rec
                .rects
                .iter()
                .find(|(_, size, _)| (size.height - MARKET_UNDERLINE_HEIGHT).abs() < 0.001)
                .copied()
                .expect("the mode underline");
            let y = bar.0.y - 8.0;
            if self.state.mode == MarketMode::Sell {
                Point::new(bar.0.x + bar.1.width / 2.0, y)
            } else {
                Point::new(bar.0.x + bar.1.width + MARKET_MODE_SPACING + 8.0, y)
            }
        }

        /// The centre of the outcome pill at `index`.
        fn outcome_pill(&mut self, index: usize) -> Point {
            let rec = self.step(0.0);
            let pill = rec
                .rrects
                .iter()
                .find(|(_, _, radius, _)| (*radius - MARKET_OUTCOME_RADIUS).abs() < 0.001)
                .copied();
            // Only the selected pill is filled, so the unselected one is
            // resolved from the selected one's own geometry.
            let (origin, size, _, _) = pill.expect("the selected pill");
            let step = size.width + MARKET_OUTCOME_GAP;
            Point::new(
                origin.x + size.width / 2.0 + (index as f64 - self.selected() as f64) * step,
                origin.y + size.height / 2.0,
            )
        }

        /// Which outcome the ticket is showing.
        fn selected(&self) -> usize {
            if self.state.outcome == "down" { 1 } else { 0 }
        }

        /// The centre of the chip at `index`; the last is `Max`.
        fn chip(&mut self, index: usize) -> Point {
            let rec = self.step(0.0);
            let chips: Vec<Rect> = rec
                .rrects
                .iter()
                .filter(|(_, size, _, _)| (size.height - MARKET_CHIP_HEIGHT).abs() < 0.001)
                .map(|(origin, size, _, _)| Rect::from_origin_size(*origin, *size))
                .collect();
            chips[index].center()
        }

        /// The action button's centre.
        fn action(&mut self) -> Point {
            let rec = self.step(0.0);
            let button = rec
                .rrects
                .iter()
                .rev()
                .find(|(_, size, _, _)| (size.height - MARKET_ACTION_HEIGHT).abs() < 0.001)
                .copied()
                .expect("the action button");
            Rect::from_origin_size(button.0, button.1).center()
        }
    }

    #[test]
    fn switching_mode_reports_the_mode_and_clears_the_amount() {
        let mut h = Harness::new();
        h.state.amount = 50.0;
        h.step(0.0);
        let at = h.sell_tab();
        h.click(at);
        assert_eq!(h.state.modes, vec![MarketMode::Sell]);
        assert_eq!(h.state.amounts, vec![0.0], "the amount was cleared with it");
        // Pressing the tab it is already on reports nothing.
        let at = h.sell_tab();
        h.click(at);
        assert_eq!(h.state.modes.len(), 1);
    }

    #[test]
    fn picking_an_outcome_reports_its_id() {
        let mut h = Harness::new();
        let at = h.outcome_pill(1);
        h.click(at);
        assert_eq!(h.state.outcomes, vec!["down".to_string()]);
    }

    #[test]
    fn a_quick_add_chip_adds_to_the_amount_rather_than_replacing_it() {
        let mut h = Harness::new();
        let at = h.chip(0);
        h.click(at);
        assert_eq!(h.state.amounts, vec![10.0]);
        let at = h.chip(1);
        h.click(at);
        assert_eq!(h.state.amounts, vec![10.0, 60.0], "50 added to the 10");
    }

    #[test]
    fn max_takes_the_balance_on_a_buy_and_the_holding_on_a_sell() {
        let mut h = Harness::new();
        let at = h.chip(4);
        h.click(at);
        assert_eq!(h.state.amounts, vec![MARKET_DEFAULT_BALANCE]);

        h.state.mode = MarketMode::Sell;
        h.state.amount = 0.0;
        h.step(0.0);
        let at = h.chip(4);
        h.click(at);
        assert_eq!(
            h.state.amounts.last().copied(),
            Some(24.0),
            "the wallet's own holding of the selected outcome"
        );
    }

    #[test]
    fn a_valid_trade_reports_and_an_invalid_one_shakes_instead() {
        let mut h = Harness::new();
        // Nothing staked: the action refuses and shakes.
        let at = h.action();
        h.click(at);
        assert_eq!(h.state.trades, 0);
        // The click's own frame latches the shake's clock; the displacement is
        // visible from the next one, and gone once the run finishes.
        let shaking = h.step(60.0);
        assert!(
            !shaking.transforms.is_empty(),
            "the refusal did not displace the ticket"
        );
        let settled = h.step(MARKET_SHAKE.as_secs_f64() * 1_000.0 + 10.0);
        assert!(settled.transforms.is_empty(), "the shake never settled");

        // A valid order reports instead.
        h.state.amount = 50.0;
        h.step(0.0);
        let at = h.action();
        h.click(at);
        assert_eq!(h.state.trades, 1);
    }

    #[test]
    fn a_placing_ticket_freezes_every_affordance_but_its_own_action() {
        let mut h = Harness::new();
        h.state.amount = 50.0;
        h.state.status = MarketStatus::Placing;
        h.step(0.0);
        let at = h.chip(0);
        h.click(at);
        assert!(h.state.amounts.is_empty(), "a chip fired while placing");
        let at = h.action();
        h.click(at);
        assert_eq!(h.state.trades, 1, "the action itself is still live");
    }

    #[test]
    fn an_unauthenticated_ticket_shows_connect_and_reports_a_sign_in() {
        let mut h = Harness::new();
        h.state.authenticated = false;
        h.step(0.0);
        let at = h.action();
        h.click(at);
        assert_eq!(h.state.sign_ins, 1);
        assert_eq!(h.state.trades, 0);
    }

    #[test]
    fn the_payout_ticker_is_the_catalog_s_own_number_and_follows_the_quote() {
        let mut h = Harness::new();
        h.state.amount = 90.0;
        // 90 at 9 cents is 1000 shares, which is what "to win" shows.
        let rec = h.step(0.0);
        assert!(!rec.inks.is_empty(), "the ticker drew nothing");
        let quote = market_quote(MarketMode::Buy, 90.0, &up(), 500.0, 1.0);
        assert!((quote.payout - 1_000.0).abs() < 1e-9);
    }

    #[test]
    fn the_amount_rolls_its_characters_in_and_the_old_ones_out() {
        let mut h = Harness::new();
        h.state.amount = 50.0;
        let rolling = h.step(0.0);
        let settled = h.step(MARKET_DIGIT_TRANSITION.as_secs_f64() * 1_000.0 + 10.0);
        assert_ne!(
            rolling.inks.len(),
            settled.inks.len(),
            "no character was mid-swap"
        );
    }

    #[test]
    fn a_reduced_motion_ticket_never_shakes_and_places_its_digits_at_once() {
        let mut h = Harness::themed(reduced());
        let at = h.action();
        h.click(at);
        let first = h.step(60.0);
        let second = h.step(16.0);
        assert!(
            first.transforms.is_empty() && second.transforms.is_empty(),
            "a reduced-motion refusal displaced the ticket"
        );
    }

    // ---- Typeface: the ticket's runs follow the live theme ------------------

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// A two-outcome ticket: modes, order type, amount, chips, outcomes, the
    /// quote line and the action, plus the payout ticker `number` paints.
    fn probe_view(_: &mut ()) -> frust::StackView<()> {
        frust::stack().child(prediction_market::<()>(vec![
            market_outcome("yes", "Yes", 0.55),
            market_outcome("no", "No", 0.45),
        ]))
    }

    #[test]
    fn ticket_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the ticket's text", probe_view, WINDOW);
        let runs = Probe::new(probe_view, WINDOW, crate::theme()).frame();
        assert_eq!(
            runs.len(),
            17,
            "every run of the ticket and its payout ticker"
        );
    }

    #[test]
    fn ticket_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the ticket's text", probe_view, WINDOW);
    }
}
