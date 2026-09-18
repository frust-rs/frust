//! Ports beUI's `swap` block — `components/motion/swap.tsx` plus its `swap/`
//! subdirectory (beUI rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved
//! 2026-09-01), registry slug `swap`, name *"Multi-chain Swap"*: *"Cross-chain
//! swap widget with chain + token selectors, morphing views, animated flip and
//! quote."*
//!
//! # A premise correction
//!
//! The porting card describes this slug as *"the swap interaction pattern
//! (content A/B swap with direction-aware spring transitions)"*. That is a
//! different upstream component — `components/motion/action-swap.tsx`, already
//! ported as [`crate::components::action_swap`]. This slug is a **crypto swap
//! widget**: a pay field over a get field, a flip button that reverses the pair,
//! a quote block, a destination-address row and an action button whose label
//! tracks the order's validity. The port follows upstream.
//!
//! | upstream | here |
//! |---|---|
//! | `max-w-[420px] rounded-3xl border bg-card` | [`SWAP_WIDTH`], [`SWAP_RADIUS`] |
//! | the `h-12` header with its settings affordance | [`SWAP_HEADER_HEIGHT`] |
//! | `Field`'s `rounded-2xl p-3.5`, "You pay" / "You get" | [`SwapSide`], [`SWAP_FIELD_PADDING`] |
//! | `FlipButton`'s `rotate: rotation` on `{stiffness 380, damping 26, mass 0.6}` | [`SWAP_FLIP`], [`SWAP_FLIP_TURN`] |
//! | its `whileTap={{ scale: 0.9 }}` | [`SWAP_FLIP_PRESS_SCALE`] |
//! | the 450ms quote settle and its `opacity 0.55` dim | [`SWAP_QUOTE_SETTLE`], [`SWAP_QUOTING_ALPHA`] |
//! | `QuoteRow`'s four label/value pairs | [`SwapQuote`] |
//! | `DestinationRow`'s `height: auto` reveal and its chevron half-turn | [`SWAP_DEST_REVEAL`] |
//! | `isValidAddress` / `truncateAddress` | [`is_valid_address`] / [`truncate_address`] |
//! | `formatAmount` (`maximumFractionDigits`, grouped past 1000) | [`format_amount`] |
//! | `ActionButton`'s four-way label and its `disabled` rule | [`swap_action_label`], [`SwapActionState`] |
//! | `active:scale-[0.97]` on the token, settings and `Max` affordances | [`SWAP_TAP_SCALE`] via `SwapWidget`'s shared `affordance_press` lane |
//!
//! The action button's own press shrink (`paint_action`) also reuses
//! [`SWAP_TAP_SCALE`] through `action_press` — this port has no separately
//! documented upstream value for the submit button, so it borrows the same
//! constant rather than inventing an undocumented one.
//!
//! # Degradations against the web original
//!
//! - **No editable amount field and no address field.** Both are `<input>`s
//!   upstream. A controlled text field is [`crate::components::input`], and the
//!   amount is the app's number either way, so the pay amount arrives through
//!   [`SwapView::amount`] and the destination through
//!   [`SwapView::destination`]. Everything else the fields do — the derived
//!   get-amount, the USD line, the balance line, `Max` — is carried, with `Max`
//!   reported through [`SwapView::on_amount_change`].
//! - **No token picker sheet.** `TokenPicker` is a searchable, chain-filtered
//!   bottom sheet; [`crate::components::bottom_sheet`] is that host, and a
//!   picker built inside this block would be a second one. The two token
//!   affordances report through [`SwapView::on_pick_token`] instead, with the
//!   [`SwapSide`] that was pressed.
//! - **No blur on the quoting dim.** `filter: blur(2px)` has no primitive; the
//!   opacity half is kept.
//! - **No spinner.** `Loader2`'s continuous rotation is
//!   [`crate::components::loader`]; the quoting state here is the dim plus the
//!   held quote row.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, Affine, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color,
    ErasedArgCallback, EventCtx, EventResult, InputEvent, LayoutCtx, PaintCtx, PaintScene, Point,
    PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2, View, Widget,
    erase_callback_arg, text::TextStyle,
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::components::popover::{PanelChrome, paint_panel_hairline, resolve_panel};
use crate::motion::Ramp;
use crate::press::{Lane, inside, is_activation_key, press_scale, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::{EASE_OUT, SPRING_PRESS};

// ---- Metrics ---------------------------------------------------------------

/// `max-w-[420px]` — the widget's width, in logical px.
pub const SWAP_WIDTH: f64 = 420.0;

/// `rounded-3xl` — its corner radius.
pub const SWAP_RADIUS: f64 = style::RADIUS_3XL;

/// `h-12` — the header's height, in logical px.
pub const SWAP_HEADER_HEIGHT: f64 = 48.0;

/// `p-4` — the body's padding, in logical px.
pub const SWAP_PADDING: f64 = 16.0;

/// `gap-1.5` — the gap between the body's stacked blocks.
pub const SWAP_GAP: f64 = 6.0;

/// `rounded-2xl p-3.5` — a field's radius and padding.
pub const SWAP_FIELD_RADIUS: f64 = style::RADIUS_2XL;

/// A field's own padding, in logical px (`p-3.5`).
pub const SWAP_FIELD_PADDING: f64 = 14.0;

/// A field's own height, in logical px — its label, its amount row and its
/// balance line inside `p-3.5`.
pub const SWAP_FIELD_HEIGHT: f64 = 112.0;

/// `text-2xl` — the amount's type size, in logical px.
pub const SWAP_AMOUNT_SIZE: f64 = 24.0;

/// `text-[11px]` — the USD and balance lines' size, in logical px.
pub const SWAP_MICRO_SIZE: f64 = 11.0;

/// `h-9 w-9 border-[3px]` — the flip button's disc and the ring it punches
/// through the two fields.
pub const SWAP_FLIP_SIZE: f64 = 36.0;

/// The width of that ring, in logical px (`border-[3px]`).
pub const SWAP_FLIP_RING: f64 = 3.0;

/// `-my-4` — how far the flip button overlaps the fields either side of it.
pub const SWAP_FLIP_OVERLAP: f64 = 16.0;

/// `h-10 rounded-full` — a token affordance's box, in logical px.
pub const SWAP_TOKEN_HEIGHT: f64 = 40.0;

/// `rounded-xl px-3.5 py-2.5` — the quote block's radius and padding.
pub const SWAP_QUOTE_RADIUS: f64 = style::RADIUS_XL;

/// One quote row's height, in logical px (`gap-y-1.5` on `text-[11px]`).
pub const SWAP_QUOTE_ROW_HEIGHT: f64 = 18.0;

/// The destination row's collapsed height, in logical px (`px-3.5 py-2.5`).
pub const SWAP_DEST_HEIGHT: f64 = 38.0;

/// Its expanded extra height, in logical px — the address field's own row.
pub const SWAP_DEST_FIELD_HEIGHT: f64 = 52.0;

/// `h-12 rounded-2xl` — the action button's box.
pub const SWAP_ACTION_HEIGHT: f64 = 48.0;

/// `bg-primary/10` — a disabled action's fill alpha.
pub const SWAP_DISABLED_FILL_ALPHA: f32 = 0.10;

// ---- Motion ----------------------------------------------------------------

/// The flip button's own spring — `{ type: "spring", stiffness: 380, damping:
/// 26, mass: 0.6 }`, a component-local constant upstream authors inline rather
/// than pulling from `@/lib/ease`, so it is carried inline here too.
pub const SWAP_FLIP: SpringDescription = SpringDescription {
    mass: 0.6,
    stiffness: 380.0,
    damping: 26.0,
};

/// `setFlipRot((r) => r + 180)` — how far the flip button turns per press, in
/// degrees.
///
/// It accumulates rather than toggling, which is what makes a second flip keep
/// turning the same way instead of unwinding.
pub const SWAP_FLIP_TURN: f64 = 180.0;

/// `whileTap={{ scale: 0.9 }}` — the flip button's press shrink.
pub const SWAP_FLIP_PRESS_SCALE: f64 = 0.9;

/// `active:scale-[0.97]` — the token, settings and `Max` affordances' press
/// shrink, driven by `SwapWidget`'s shared `affordance_press` lane. The
/// action button's own press shrink (`action_press`) also borrows this
/// constant; see the upstream-mapping table at the top of the module.
pub const SWAP_TAP_SCALE: f64 = 0.97;

/// `setTimeout(() => setQuoting(false), 450)` — how long a re-quote holds the
/// get side dim.
pub const SWAP_QUOTE_SETTLE: Duration = Duration::from_millis(450);

/// `opacity: quoting ? 0.55 : 1` — the get amount's alpha while re-quoting.
pub const SWAP_QUOTING_ALPHA: f64 = 0.55;

/// `duration: 0.22` — the destination row's reveal.
pub const SWAP_DEST_REVEAL: Duration = Duration::from_millis(220);

/// `duration: 0.2` — its chevron's half-turn.
pub const SWAP_CHEVRON_TURN: Duration = Duration::from_millis(200);

// ---- Formatting and validation ---------------------------------------------

/// `isValidAddress`: a 0x-prefixed 40-hex-digit address, or a name over five
/// characters ending in a supported TLD.
pub fn is_valid_address(value: &str) -> bool {
    if let Some(hex) = value.strip_prefix("0x")
        && hex.len() == 40
        && hex.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return true;
    }
    value.len() > 5
        && [".eth", ".sol", ".bnb"]
            .iter()
            .any(|tld| value.ends_with(tld))
}

/// `truncateAddress`: a full 0x address becomes `0x1234...cdef`; anything else
/// is shown as it is.
///
/// Deliberately not the wallet card's own truncation, which is a different rule
/// on a different length — upstream ships two, and folding them would change
/// one component's output.
///
/// Counted in **characters, not bytes** (the `wallet_card.rs` pattern), so a
/// caller-supplied value that happens to be 42 bytes but not 42 characters —
/// or 42 characters with a multibyte one among the first six or last four —
/// is cut on a char boundary rather than mid-codepoint, which a raw byte
/// slice cannot guarantee for arbitrary text.
pub fn truncate_address(value: &str) -> String {
    if !value.starts_with("0x") {
        return value.to_string();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() != 42 {
        return value.to_string();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}...{tail}")
}

/// `formatAmount(n, max = 6)`: `0` for zero and anything non-finite, at most two
/// fraction digits with thousands grouped from 1000 up, at most `max` below it.
///
/// Trailing zeros are dropped, matching `toLocaleString`'s own `maximum-` (not
/// `minimum-`) `FractionDigits`. The group separator is an ASCII comma for the
/// reason [`crate::components::number`] records: the facade publishes no locale
/// seam.
pub fn format_amount(value: f64, max: usize) -> String {
    if !value.is_finite() || value == 0.0 {
        return "0".to_string();
    }
    let digits = if value.abs() >= 1_000.0 { 2 } else { max };
    let fixed = format!("{:.*}", digits, value.abs());
    let (whole, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), ""));
    let trimmed = fraction.trim_end_matches('0');
    let mut grouped = String::new();
    for (index, ch) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    if trimmed.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{trimmed}")
    }
}

// ---- Tokens and sides ------------------------------------------------------

/// Which half of the pair an affordance belongs to — upstream's `TokenSide`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapSide {
    /// The top field: *"You pay"*.
    From,
    /// The bottom field: *"You get"*.
    To,
}

impl SwapSide {
    /// The field's own label.
    pub const fn label(self) -> &'static str {
        match self {
            SwapSide::From => "You pay",
            SwapSide::To => "You get",
        }
    }

    /// The other side.
    pub const fn other(self) -> Self {
        match self {
            SwapSide::From => SwapSide::To,
            SwapSide::To => SwapSide::From,
        }
    }
}

/// One token on one chain — upstream's `Token` joined with its `Chain`, since a
/// token is never shown apart from its chain here.
#[derive(Clone, Debug, PartialEq)]
pub struct SwapToken {
    symbol: String,
    chain: String,
    balance: f64,
    usd: f64,
}

/// A token with `symbol` on `chain`.
pub fn swap_token(symbol: impl Into<String>, chain: impl Into<String>) -> SwapToken {
    SwapToken {
        symbol: symbol.into(),
        chain: chain.into(),
        balance: 0.0,
        usd: 0.0,
    }
}

impl SwapToken {
    /// Set the wallet's holding of this token.
    pub fn balance(mut self, balance: f64) -> Self {
        self.balance = balance;
        self
    }

    /// Set the token's price in USD.
    pub fn usd(mut self, usd: f64) -> Self {
        self.usd = usd;
        self
    }

    /// The token's ticker.
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// The chain it lives on.
    pub fn chain(&self) -> &str {
        &self.chain
    }

    /// The wallet's holding.
    pub fn token_balance(&self) -> f64 {
        self.balance
    }

    /// Its price in USD.
    pub fn token_usd(&self) -> f64 {
        self.usd
    }
}

/// The rate between two tokens — `from.usd / to.usd`, with a missing price on
/// either side falling back to parity, exactly as upstream's memo does.
pub fn swap_rate(from: &SwapToken, to: &SwapToken) -> f64 {
    if from.usd <= 0.0 || to.usd <= 0.0 {
        return 1.0;
    }
    from.usd / to.usd
}

/// The quote block's four rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwapQuote {
    /// The network fee, in USD.
    pub fee: f64,
    /// The slippage tolerance, as a percentage.
    pub slippage: f64,
}

impl Default for SwapQuote {
    /// Upstream's own demo values: `fee={0.42} slippage={0.5}`.
    fn default() -> Self {
        SwapQuote {
            fee: 0.42,
            slippage: 0.5,
        }
    }
}

/// What the action button is saying, which is also whether it may be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapActionState {
    /// `amount <= 0`: *"Enter an amount"*, and unpressable.
    NoAmount,
    /// The amount is over the pay token's balance: *"Insufficient …"*, and
    /// unpressable.
    OverBalance,
    /// A valid destination is set: *"Swap + Send to …"*.
    SendTo,
    /// The ordinary case: *"Swap A → B"*.
    Ready,
}

impl SwapActionState {
    /// Whether the action may be pressed — upstream's `disabled = noAmount ||
    /// overBalance`.
    pub const fn is_enabled(self) -> bool {
        matches!(self, SwapActionState::SendTo | SwapActionState::Ready)
    }
}

/// Which state an order is in, in upstream's own precedence: no amount first,
/// then the balance check, then a valid destination, then the plain swap.
pub fn swap_action_state(amount: f64, from: &SwapToken, destination: &str) -> SwapActionState {
    if amount <= 0.0 {
        return SwapActionState::NoAmount;
    }
    if from.balance > 0.0 && amount > from.balance {
        return SwapActionState::OverBalance;
    }
    if !destination.is_empty() && is_valid_address(destination) {
        return SwapActionState::SendTo;
    }
    SwapActionState::Ready
}

/// The action button's label for `state`.
pub fn swap_action_label(
    state: SwapActionState,
    from: &SwapToken,
    to: &SwapToken,
    destination: &str,
) -> String {
    match state {
        SwapActionState::NoAmount => "Enter an amount".to_string(),
        SwapActionState::OverBalance => format!("Insufficient {}", from.symbol),
        SwapActionState::SendTo => {
            format!("Swap + Send to {}", truncate_address(destination))
        }
        SwapActionState::Ready => format!("Swap {} → {}", from.symbol, to.symbol),
    }
}

// ---- The component ---------------------------------------------------------

/// A view-held side callback (erased on build).
type OnSide<State> = Rc<dyn Fn(&mut State, SwapSide)>;

/// A view-held amount callback (erased on build).
type OnAmount<State> = Rc<dyn Fn(&mut State, f64)>;

/// A view-held flag callback (erased on build).
type OnFlag<State> = Rc<dyn Fn(&mut State, bool)>;

/// What the widget renders from.
#[derive(Clone, Debug, PartialEq)]
struct SwapConfig {
    from: SwapToken,
    to: SwapToken,
    amount: f64,
    destination: String,
    show_destination: Option<bool>,
    quote: SwapQuote,
    eta: String,
    title: String,
}

/// A declarative beUI multi-chain swap. See [`swap`].
pub struct SwapView<State: 'static> {
    config: SwapConfig,
    on_flip: Rc<dyn Fn(&mut State)>,
    on_pick_token: OnSide<State>,
    on_amount_change: OnAmount<State>,
    on_destination_toggle: OnFlag<State>,
    on_submit: Rc<dyn Fn(&mut State)>,
}

/// Build a swap widget paying `from` and receiving `to`.
pub fn swap<State: 'static>(from: SwapToken, to: SwapToken) -> SwapView<State> {
    SwapView {
        config: SwapConfig {
            from,
            to,
            amount: 0.0,
            destination: String::new(),
            show_destination: None,
            quote: SwapQuote::default(),
            eta: "≈ 24s".to_string(),
            title: "Swap".to_string(),
        },
        on_flip: Rc::new(|_| {}),
        on_pick_token: Rc::new(|_, _| {}),
        on_amount_change: Rc::new(|_, _| {}),
        on_destination_toggle: Rc::new(|_, _| {}),
        on_submit: Rc::new(|_| {}),
    }
}

impl<State: 'static> SwapView<State> {
    /// Set the amount being paid.
    pub fn amount(mut self, amount: f64) -> Self {
        self.config.amount = if amount.is_finite() { amount } else { 0.0 };
        self
    }

    /// Set the destination address (`destAddress`).
    pub fn destination(mut self, destination: impl Into<String>) -> Self {
        self.config.destination = destination.into();
        self
    }

    /// Take the destination row's expansion over (`showDest`).
    pub fn show_destination(mut self, show: bool) -> Self {
        self.config.show_destination = Some(show);
        self
    }

    /// Set the quote block's fee and slippage.
    pub fn quote(mut self, quote: SwapQuote) -> Self {
        self.config.quote = quote;
        self
    }

    /// Set the quote block's ETA line (`eta`).
    pub fn eta(mut self, eta: impl Into<String>) -> Self {
        self.config.eta = eta.into();
        self
    }

    /// Set the header's title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.config.title = title.into();
        self
    }

    /// Set the callback the flip button reports through — the app swaps the
    /// pair, which is what upstream's own `flip` does.
    pub fn on_flip<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_flip = Rc::new(f);
        self
    }

    /// Set the callback a token affordance reports through (upstream's
    /// `setPicking`).
    pub fn on_pick_token<F: Fn(&mut State, SwapSide) + 'static>(mut self, f: F) -> Self {
        self.on_pick_token = Rc::new(f);
        self
    }

    /// Set the callback `Max` reports the new amount through.
    pub fn on_amount_change<F: Fn(&mut State, f64) + 'static>(mut self, f: F) -> Self {
        self.on_amount_change = Rc::new(f);
        self
    }

    /// Set the callback the destination row's own toggle reports through.
    pub fn on_destination_toggle<F: Fn(&mut State, bool) + 'static>(mut self, f: F) -> Self {
        self.on_destination_toggle = Rc::new(f);
        self
    }

    /// Set the callback the action button reports through.
    pub fn on_submit<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_submit = Rc::new(f);
        self
    }
}

/// Which affordance a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The header's settings affordance.
    Settings,
    /// One of the two token affordances.
    Token(SwapSide),
    /// The pay field's `Max`.
    Max,
    /// The flip button.
    Flip,
    /// The destination row's own toggle.
    Destination,
    /// The action button.
    Submit,
}

/// The retained widget for a [`SwapView`].
pub struct SwapWidget {
    config: SwapConfig,
    title: LabelRun,
    labels: [LabelRun; 2],
    amounts: [LabelRun; 2],
    usd: [LabelRun; 2],
    balances: [LabelRun; 2],
    symbols: [LabelRun; 2],
    max: LabelRun,
    quote_labels: [LabelRun; 4],
    quote_values: [LabelRun; 4],
    destination_line: LabelRun,
    destination_value: LabelRun,
    action: LabelRun,
    /// The flip button's accumulated rotation, in degrees.
    turn: f64,
    /// The lane carrying it there.
    flip: Lane,
    /// The destination row's reveal: `0.0` collapsed, `1.0` open.
    reveal: Lane,
    /// Its chevron's half-turn.
    chevron: Lane,
    /// The flip button's press shrink.
    flip_press: Lane,
    /// The action button's press shrink.
    action_press: Lane,
    /// The token/settings/`Max` affordances' shared press shrink — only one
    /// of them can be armed at a time (a single `armed: Option<Target>`
    /// drives all presses), so one lane covers the whole group.
    affordance_press: Lane,
    /// Which affordance `affordance_press` is currently shrinking, so paint
    /// applies the value to the right box during the release decay (the
    /// lane itself no longer knows once `armed` is cleared on `Up`/`Cancel`).
    affordance_press_target: Option<Target>,
    /// Whether the destination row is open, as this widget last resolved it.
    show_destination: bool,
    /// The frame the current re-quote started on, or `None` once it settled.
    quoting_since: Option<FrameTime>,
    /// Set by `rebuild` when the order changed: the re-quote needs a clock, and
    /// only `paint` has one.
    quote_pending: bool,
    /// The order's own state, resolved at layout.
    state: SwapActionState,
    /// The boxes the last layout resolved, in the widget's own space.
    fields: [Rect; 2],
    tokens: [Rect; 2],
    settings: Rect,
    max_box: Rect,
    flip_box: Rect,
    quote_box: Rect,
    destination_box: Rect,
    action_box: Rect,
    /// The affordance a `Down` armed.
    armed: Option<Target>,
    on_flip: frust::authoring::ErasedCallback,
    on_pick_token: ErasedArgCallback<SwapSide>,
    on_amount_change: ErasedArgCallback<f64>,
    on_destination_toggle: ErasedArgCallback<bool>,
    on_submit: frust::authoring::ErasedCallback,
}

impl SwapWidget {
    /// The flip button's accumulated rotation, in degrees.
    pub fn turn(&self) -> f64 {
        self.turn
    }

    /// Whether the get side is dim because a new quote is in flight.
    pub fn is_quoting(&self) -> bool {
        self.quoting_since.is_some() || self.quote_pending
    }

    /// Whether the destination row is expanded.
    pub fn shows_destination(&self) -> bool {
        self.show_destination
    }

    /// The order's own state — what the action button says, and whether it may
    /// be pressed.
    pub fn action_state(&self) -> SwapActionState {
        self.state
    }

    /// The amount the get side shows: the pay amount at the pair's rate.
    pub fn received(&self) -> f64 {
        self.config.amount * swap_rate(&self.config.from, &self.config.to)
    }

    /// The affordance `position` lands on, if any.
    fn target_at(&self, position: Point) -> Option<Target> {
        if self.flip_box.contains(position) {
            return Some(Target::Flip);
        }
        if self.settings.contains(position) {
            return Some(Target::Settings);
        }
        if self.max_box.width() > 0.0 && self.max_box.contains(position) {
            return Some(Target::Max);
        }
        if self.tokens[0].contains(position) {
            return Some(Target::Token(SwapSide::From));
        }
        if self.tokens[1].contains(position) {
            return Some(Target::Token(SwapSide::To));
        }
        // Only the row's own header toggles it; the revealed field below is
        // the app's to fill.
        if self.destination_box.contains(position)
            && position.y <= self.destination_box.y0 + SWAP_DEST_HEIGHT
        {
            return Some(Target::Destination);
        }
        if self.action_box.contains(position) {
            return Some(Target::Submit);
        }
        None
    }

    /// Act on a released press.
    fn fire(&mut self, ctx: &mut EventCtx, target: Target) {
        match target {
            // Upstream's settings affordance opens nothing; it is chrome, and
            // a port that invented a panel for it would be inventing a
            // component.
            Target::Settings => {}
            Target::Token(side) => (self.on_pick_token)(ctx, side),
            Target::Max => {
                let balance = self.config.from.balance;
                if balance > 0.0 {
                    (self.on_amount_change)(ctx, balance);
                }
            }
            Target::Flip => {
                // The rotation accumulates rather than toggling, so a second
                // flip keeps turning the same way.
                self.turn += SWAP_FLIP_TURN;
                self.flip.retarget(self.turn);
                (self.on_flip)(ctx);
            }
            Target::Destination => {
                let next = !self.show_destination;
                if self.config.show_destination.is_none() {
                    self.set_destination(next);
                }
                (self.on_destination_toggle)(ctx, next);
            }
            Target::Submit => {
                if self.state.is_enabled() {
                    (self.on_submit)(ctx);
                }
            }
        }
        ctx.request_redraw();
    }

    /// Apply the destination row's expansion. Reports whether it changed.
    fn set_destination(&mut self, show: bool) -> bool {
        if self.show_destination == show {
            return false;
        }
        self.show_destination = show;
        self.reveal.retarget(if show { 1.0 } else { 0.0 });
        self.chevron.retarget(if show { 1.0 } else { 0.0 });
        true
    }

    /// The get side's alpha: dim while a re-quote is in flight.
    fn quote_alpha(&self, now: FrameTime) -> f64 {
        match self.quoting_since {
            Some(started) if now.saturating_sub(started) < SWAP_QUOTE_SETTLE => SWAP_QUOTING_ALPHA,
            _ => 1.0,
        }
    }

    /// `SWAP_TAP_SCALE` for `target` if it is the affordance
    /// `affordance_press` is currently shrinking, `1.0` (at rest) otherwise —
    /// the token boxes, the settings glyph and `Max` all read this.
    fn affordance_scale(&self, target: Target) -> f64 {
        if self.affordance_press_target == Some(target) {
            press_scale(SWAP_TAP_SCALE, self.affordance_press.value())
        } else {
            1.0
        }
    }
}

/// The four quote-row labels, in upstream's own order.
const QUOTE_LABELS: [&str; 4] = ["Rate", "Network fee", "Slippage", "ETA"];

impl<State: 'static> View<State> for SwapView<State> {
    type Element = SwapWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SwapWidget {
        let show = self.config.show_destination.unwrap_or(false);
        let state = swap_action_state(
            self.config.amount,
            &self.config.from,
            &self.config.destination,
        );
        SwapWidget {
            title: LabelRun::new(self.config.title.clone()),
            labels: [
                LabelRun::new(SwapSide::From.label()),
                LabelRun::new(SwapSide::To.label()),
            ],
            amounts: [LabelRun::new("0"), LabelRun::new("0")],
            usd: [LabelRun::new(""), LabelRun::new("")],
            balances: [LabelRun::new(""), LabelRun::new("")],
            symbols: [
                LabelRun::new(self.config.from.symbol.clone()),
                LabelRun::new(self.config.to.symbol.clone()),
            ],
            max: LabelRun::new("Max"),
            quote_labels: QUOTE_LABELS.map(LabelRun::new),
            quote_values: [
                LabelRun::new(""),
                LabelRun::new(""),
                LabelRun::new(""),
                LabelRun::new(self.config.eta.clone()),
            ],
            destination_line: LabelRun::new("Send to different address"),
            destination_value: LabelRun::new(self.config.destination.clone()),
            action: LabelRun::new(swap_action_label(
                state,
                &self.config.from,
                &self.config.to,
                &self.config.destination,
            )),
            config: self.config.clone(),
            turn: 0.0,
            flip: Lane::at_rest(Ramp::spring(SWAP_FLIP), 0.0),
            reveal: Lane::at_rest(
                Ramp::eased(SWAP_DEST_REVEAL, EASE_OUT),
                if show { 1.0 } else { 0.0 },
            ),
            chevron: Lane::at_rest(
                Ramp::eased(SWAP_CHEVRON_TURN, EASE_OUT),
                if show { 1.0 } else { 0.0 },
            ),
            flip_press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            action_press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            affordance_press: Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            affordance_press_target: None,
            show_destination: show,
            quoting_since: None,
            quote_pending: false,
            state,
            fields: [Rect::ZERO; 2],
            tokens: [Rect::ZERO; 2],
            settings: Rect::ZERO,
            max_box: Rect::ZERO,
            flip_box: Rect::ZERO,
            quote_box: Rect::ZERO,
            destination_box: Rect::ZERO,
            action_box: Rect::ZERO,
            armed: None,
            on_flip: frust::authoring::erase_callback(&self.on_flip),
            on_pick_token: erase_callback_arg(&self.on_pick_token),
            on_amount_change: erase_callback_arg(&self.on_amount_change),
            on_destination_toggle: erase_callback_arg(&self.on_destination_toggle),
            on_submit: frust::authoring::erase_callback(&self.on_submit),
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut SwapWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if element.config != self.config {
            // Upstream re-quotes on `${amount}:${fromId}:${toId}` — a changed
            // amount or either token, and nothing else.
            let requote = element.config.amount != self.config.amount
                || element.config.from != self.config.from
                || element.config.to != self.config.to;
            if requote && self.config.amount != 0.0 {
                element.quote_pending = true;
            }
            if let Some(show) = self.config.show_destination {
                element.set_destination(show);
            }
            element.title.set_content(self.config.title.clone());
            element.symbols[0].set_content(self.config.from.symbol.clone());
            element.symbols[1].set_content(self.config.to.symbol.clone());
            element.quote_values[3].set_content(self.config.eta.clone());
            element
                .destination_value
                .set_content(self.config.destination.clone());
            element.config = self.config.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_flip = frust::authoring::erase_callback(&self.on_flip);
        element.on_pick_token = erase_callback_arg(&self.on_pick_token);
        element.on_amount_change = erase_callback_arg(&self.on_amount_change);
        element.on_destination_toggle = erase_callback_arg(&self.on_destination_toggle);
        element.on_submit = frust::authoring::erase_callback(&self.on_submit);
        flags
    }

    fn teardown(&self, _element: &mut SwapWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// One label style at `size`, in the theme's `label_large` family.
fn swap_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    themed_style(
        crate::text::label_style(size),
        ThemeTextType::LabelLarge,
        theme,
    )
}

impl Widget for SwapWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        // Every style is resolved before the first `layout` call: the shaper
        // takes `ctx` mutably, and the theme read borrows it.
        let body = swap_style(theme, style::TEXT_SM);
        let micro = swap_style(theme, SWAP_MICRO_SIZE);
        let big = swap_style(theme, SWAP_AMOUNT_SIZE);

        // The two sides' derived content.
        let received = self.received();
        self.state = swap_action_state(
            self.config.amount,
            &self.config.from,
            &self.config.destination,
        );
        let sides = [
            (self.config.amount, &self.config.from),
            (received, &self.config.to),
        ];
        for (index, (amount, token)) in sides.into_iter().enumerate() {
            self.amounts[index].set_content(if amount > 0.0 {
                format_amount(amount, 6)
            } else {
                "0".to_string()
            });
            self.usd[index].set_content(format!("≈ ${}", format_amount(amount * token.usd, 2)));
            self.balances[index].set_content(format!(
                "{} · {}",
                if token.balance > 0.0 {
                    format_amount(token.balance, 6)
                } else {
                    "0.00".to_string()
                },
                token.chain
            ));
        }
        let rate = swap_rate(&self.config.from, &self.config.to);
        self.quote_values[0].set_content(format!(
            "1 {} ≈ {} {}",
            self.config.from.symbol,
            format_amount(rate, 6),
            self.config.to.symbol
        ));
        self.quote_values[1].set_content(format!("${:.2}", self.config.quote.fee));
        self.quote_values[2].set_content(format!("{:.2}%", self.config.quote.slippage));
        self.action.set_content(swap_action_label(
            self.state,
            &self.config.from,
            &self.config.to,
            &self.config.destination,
        ));
        self.destination_line.set_content(
            if self.show_destination && is_valid_address(&self.config.destination) {
                format!("To: {}", truncate_address(&self.config.destination))
            } else {
                "Send to different address".to_string()
            },
        );

        self.title.layout(ctx, &body);
        self.max.layout(ctx, &micro);
        self.destination_line.layout(ctx, &micro);
        self.destination_value.layout(ctx, &micro);
        self.action.layout(ctx, &body);
        for index in 0..2 {
            self.labels[index].layout(ctx, &micro);
            self.amounts[index].layout(ctx, &big);
            self.usd[index].layout(ctx, &micro);
            self.balances[index].layout(ctx, &micro);
            self.symbols[index].layout(ctx, &body);
        }
        for index in 0..4 {
            self.quote_labels[index].layout(ctx, &micro);
            self.quote_values[index].layout(ctx, &micro);
        }

        let width = bc.constrain(Size::new(SWAP_WIDTH, 0.0)).width;
        let inner = (width - SWAP_PADDING * 2.0).max(0.0);

        self.settings = Rect::from_origin_size(
            Point::new(
                width - style::GAP_MD - style::SIZE_ICON_BUTTON,
                (SWAP_HEADER_HEIGHT - style::SIZE_ICON_BUTTON) / 2.0,
            ),
            Size::new(style::SIZE_ICON_BUTTON, style::SIZE_ICON_BUTTON),
        );

        let mut y = SWAP_HEADER_HEIGHT + SWAP_PADDING;
        for index in 0..2 {
            self.fields[index] = Rect::from_origin_size(
                Point::new(SWAP_PADDING, y),
                Size::new(inner, SWAP_FIELD_HEIGHT),
            );
            let symbol = self.symbols[index].size();
            self.tokens[index] = Rect::from_origin_size(
                Point::new(
                    SWAP_PADDING + inner
                        - SWAP_FIELD_PADDING
                        - (symbol.width + style::PADDING_X_MD * 2.0),
                    y + SWAP_FIELD_PADDING + style::GAP_MD,
                ),
                Size::new(symbol.width + style::PADDING_X_MD * 2.0, SWAP_TOKEN_HEIGHT),
            );
            y += SWAP_FIELD_HEIGHT + SWAP_GAP;
        }
        // The flip button straddles the seam between the two fields.
        let seam = self.fields[0].y1 + SWAP_GAP / 2.0;
        self.flip_box = Rect::from_origin_size(
            Point::new(
                SWAP_PADDING + (inner - SWAP_FLIP_SIZE) / 2.0,
                seam - SWAP_FLIP_SIZE / 2.0,
            ),
            Size::new(SWAP_FLIP_SIZE, SWAP_FLIP_SIZE),
        );
        // `Max` sits on the pay field's own balance line.
        let max = self.max.size();
        self.max_box = if self.config.from.balance > 0.0 {
            Rect::from_origin_size(
                Point::new(
                    SWAP_PADDING + inner - SWAP_FIELD_PADDING - max.width - style::GAP_SM,
                    self.fields[0].y1 - SWAP_FIELD_PADDING - max.height - 2.0,
                ),
                Size::new(max.width + style::GAP_SM, max.height + 4.0),
            )
        } else {
            Rect::ZERO
        };

        // The quote block, the destination row and the action button.
        let quote_height = SWAP_QUOTE_ROW_HEIGHT * 4.0 + style::GAP_MD * 2.0;
        y += style::GAP_MD;
        self.quote_box =
            Rect::from_origin_size(Point::new(SWAP_PADDING, y), Size::new(inner, quote_height));
        y += quote_height + style::GAP_SM;
        let revealed = self.reveal.value().clamp(0.0, 1.0);
        self.destination_box = Rect::from_origin_size(
            Point::new(SWAP_PADDING, y),
            Size::new(inner, SWAP_DEST_HEIGHT + SWAP_DEST_FIELD_HEIGHT * revealed),
        );
        y = self.destination_box.y1 + style::GAP_MD;
        self.action_box = Rect::from_origin_size(
            Point::new(SWAP_PADDING, y),
            Size::new(inner, SWAP_ACTION_HEIGHT),
        );

        bc.constrain(Size::new(width, self.action_box.y1 + SWAP_PADDING))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let (accent, on_accent, wash) = match theme {
            Some(t) => (
                t.scheme().primary,
                t.scheme().on_primary,
                t.scheme().surface_container,
            ),
            None => (
                crate::BEUI_LIGHT.primary,
                crate::BEUI_LIGHT.primary_foreground,
                crate::BEUI_LIGHT.muted,
            ),
        };
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        // The re-quote's clock is latched here, because only paint has one.
        if std::mem::take(&mut self.quote_pending) {
            self.quoting_since = Some(now);
        }
        if let Some(started) = self.quoting_since {
            if now.saturating_sub(started) >= SWAP_QUOTE_SETTLE {
                self.quoting_since = None;
            } else {
                ctx.request_frame();
            }
        }
        // The destination reveal changes the widget's own height, so it is a
        // layout animation rather than a repaint. `Lane::advance` snaps to
        // the target and returns `false` on the very frame it settles, which
        // would silently skip that final relayout, and the `reduce_motion`
        // snap below has no `advance` call at all to gate on — so the
        // request is decided from a before/after compare instead, the same
        // pattern `tool_result.rs`'s own reveal lane uses.
        let reveal_before = self.reveal.value();
        let moving = if reduce {
            self.flip.snap();
            self.reveal.snap();
            self.chevron.snap();
            self.flip_press.snap();
            self.action_press.snap();
            self.affordance_press.snap();
            false
        } else {
            let mut animating = self.flip.advance(now);
            animating |= self.chevron.advance(now);
            animating |= self.flip_press.advance(now);
            animating |= self.action_press.advance(now);
            animating |= self.affordance_press.advance(now);
            if animating {
                ctx.request_frame();
            }
            self.reveal.advance(now)
        };
        if moving || self.reveal.value() != reveal_before {
            ctx.request_layout();
        }

        let radius = style::resolve_radius(SWAP_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, chrome.surface);
        paint_panel_hairline(scene, origin, size, radius, chrome.border);

        // The header.
        let title = self.title.size();
        self.title.paint(
            origin
                + Vec2::new(
                    style::GAP_MD + style::GAP_SM,
                    (SWAP_HEADER_HEIGHT - title.height) / 2.0,
                ),
            chrome.ink,
            scene,
        );
        draw_settings(
            scene,
            origin + self.settings.center().to_vec2(),
            style::ICON_SIZE * self.affordance_scale(Target::Settings),
            chrome.dim_ink,
        );
        scene.fill_rect(
            origin + Vec2::new(0.0, SWAP_HEADER_HEIGHT),
            Size::new(size.width, style::BORDER_WIDTH),
            chrome.border,
        );

        // The two fields.
        let quote_alpha = self.quote_alpha(now);
        for index in 0..2 {
            let alpha = if index == 1 { quote_alpha } else { 1.0 };
            self.paint_field(scene, origin, index, chrome, wash, alpha);
        }

        // The flip button, punched through both fields by its own ring.
        self.paint_flip(scene, origin, chrome, accent, wash);

        // The quote block, the destination row and the action button.
        self.paint_quote(scene, origin, chrome, wash, quote_alpha);
        self.paint_destination(scene, origin, chrome, wash);
        self.paint_action(scene, origin, chrome, accent, on_accent);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() || !is_activation_key(key) {
                    return EventResult::Ignored;
                }
                // A bare activation is the order itself, which is what the
                // widget's own tab stop stands for.
                self.fire(ctx, Target::Submit);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let title = self.title.content().to_string();
        let action = self.action.content().to_string();
        let enabled = self.state.is_enabled();
        let show = self.show_destination;
        ctx.push_container(
            Role::Form,
            move |node| {
                node.set_label(title.as_str());
            },
            |ctx| {
                for index in 0..2 {
                    let label = format!(
                        "{}, {} {}",
                        self.labels[index].content(),
                        self.amounts[index].content(),
                        self.symbols[index].content()
                    );
                    ctx.push_node(Role::Label, |node| {
                        node.set_label(label.as_str());
                    });
                    let token = format!("Choose token, {}", self.symbols[index].content());
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(token.as_str());
                        node.add_action(Action::Click);
                    });
                }
                ctx.push_node(Role::Button, |node| {
                    node.set_label("Reverse direction");
                    node.add_action(Action::Click);
                });
                for index in 0..4 {
                    let row = format!(
                        "{}: {}",
                        self.quote_labels[index].content(),
                        self.quote_values[index].content()
                    );
                    ctx.push_node(Role::Label, |node| {
                        node.set_label(row.as_str());
                    });
                }
                let destination = self.destination_line.content().to_string();
                ctx.push_node(Role::Button, |node| {
                    node.set_label(destination.as_str());
                    node.set_expanded(show);
                    node.add_action(Action::Click);
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label(action.as_str());
                    if enabled {
                        node.add_action(Action::Click);
                    } else {
                        node.set_disabled();
                    }
                });
            },
        );
    }
}

impl SwapWidget {
    /// One field: its wash, its label, its amount, its USD line, its token
    /// affordance and its balance line.
    fn paint_field(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        index: usize,
        chrome: PanelChrome,
        wash: Color,
        alpha: f64,
    ) {
        let rect = self.fields[index];
        let at = origin + rect.origin().to_vec2();
        scene.fill_rounded_rect(at, rect.size(), SWAP_FIELD_RADIUS, wash);
        paint_panel_hairline(scene, at, rect.size(), SWAP_FIELD_RADIUS, chrome.border);

        let label = self.labels[index].size();
        self.labels[index].paint(
            at + Vec2::new(SWAP_FIELD_PADDING, SWAP_FIELD_PADDING),
            chrome.dim_ink,
            scene,
        );
        let amount_y = at.y + SWAP_FIELD_PADDING + label.height + style::GAP_SM;
        let ink = style::with_alpha(chrome.ink, alpha.clamp(0.0, 1.0) as f32);
        self.amounts[index].paint(Point::new(at.x + SWAP_FIELD_PADDING, amount_y), ink, scene);
        let amount = self.amounts[index].size();
        self.usd[index].paint(
            Point::new(at.x + SWAP_FIELD_PADDING, amount_y + amount.height + 2.0),
            chrome.dim_ink,
            scene,
        );

        // The token affordance, shrinking on its own press (`SWAP_TAP_SCALE`,
        // documented under `active:scale-[0.97]`).
        let side = if index == 0 {
            SwapSide::From
        } else {
            SwapSide::To
        };
        let token_scale = self.affordance_scale(Target::Token(side));
        let token = self.tokens[index];
        let token_at = origin + token.origin().to_vec2();
        let token_box_size = Size::new(token.width() * token_scale, token.height() * token_scale);
        let token_box_at = token_at
            + Vec2::new(
                (token.width() - token_box_size.width) / 2.0,
                (token.height() - token_box_size.height) / 2.0,
            );
        let token_radius = style::resolve_radius(
            style::RADIUS_CONTROL,
            token_box_size.width,
            token_box_size.height,
        );
        scene.fill_rounded_rect(token_box_at, token_box_size, token_radius, chrome.surface);
        paint_panel_hairline(
            scene,
            token_box_at,
            token_box_size,
            token_radius,
            chrome.border,
        );
        let symbol = self.symbols[index].size();
        self.symbols[index].paint(
            token_box_at
                + Vec2::new(
                    (token_box_size.width - symbol.width) / 2.0,
                    (token_box_size.height - symbol.height) / 2.0,
                ),
            chrome.ink,
            scene,
        );

        // The balance line, and `Max` on the pay side.
        let balance = self.balances[index].size();
        self.balances[index].paint(
            Point::new(
                at.x + SWAP_FIELD_PADDING,
                at.y + rect.height() - SWAP_FIELD_PADDING - balance.height,
            ),
            chrome.dim_ink,
            scene,
        );
        if index == 0 && self.max_box.width() > 0.0 {
            let max_scale = self.affordance_scale(Target::Max);
            let max_centre = origin + self.max_box.center().to_vec2();
            // Scale the Max label about its box centre, not just offset it.
            scene.push_transform(Affine::scale_about(max_scale, max_centre));
            let max = self.max.size();
            self.max.paint(
                origin
                    + Vec2::new(
                        self.max_box.x0 + (self.max_box.width() - max.width) / 2.0,
                        self.max_box.y0 + (self.max_box.height() - max.height) / 2.0,
                    ),
                chrome.dim_ink,
                scene,
            );
            scene.pop_transform();
        }
    }

    /// The flip button: its punched ring, its disc and its turning mark.
    fn paint_flip(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        accent: Color,
        wash: Color,
    ) {
        let scale = press_scale(SWAP_FLIP_PRESS_SCALE, self.flip_press.value());
        let disc = SWAP_FLIP_SIZE * scale;
        let centre = origin + self.flip_box.center().to_vec2();
        // `border-[3px] border-card`: the ring is the card's own surface, which
        // is what makes the button read as punched through the field seam.
        let ring = disc + SWAP_FLIP_RING * 2.0;
        scene.fill_rounded_rect(
            Point::new(centre.x - ring / 2.0, centre.y - ring / 2.0),
            Size::new(ring, ring),
            ring / 2.0,
            chrome.surface,
        );
        scene.fill_rounded_rect(
            Point::new(centre.x - disc / 2.0, centre.y - disc / 2.0),
            Size::new(disc, disc),
            disc / 2.0,
            style::with_alpha(accent, SWAP_DISABLED_FILL_ALPHA),
        );
        let _ = wash;
        scene.push_transform(Affine::rotate_about(self.flip.value().to_radians(), centre));
        draw_arrows(scene, centre, style::ICON_SIZE * scale, chrome.ink);
        scene.pop_transform();
    }

    /// The quote block: four label/value pairs, dimmed while re-quoting.
    fn paint_quote(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        wash: Color,
        alpha: f64,
    ) {
        let rect = self.quote_box;
        if rect.height() <= 0.0 {
            return;
        }
        let at = origin + rect.origin().to_vec2();
        scene.fill_rounded_rect(at, rect.size(), SWAP_QUOTE_RADIUS, wash);
        paint_panel_hairline(scene, at, rect.size(), SWAP_QUOTE_RADIUS, chrome.border);
        let ink = style::with_alpha(chrome.ink, alpha.clamp(0.0, 1.0) as f32);
        for index in 0..4 {
            let y = at.y + style::GAP_MD + index as f64 * SWAP_QUOTE_ROW_HEIGHT;
            self.quote_labels[index].paint(
                Point::new(at.x + style::PADDING_X_SM, y),
                chrome.dim_ink,
                scene,
            );
            let value = self.quote_values[index].size();
            self.quote_values[index].paint(
                Point::new(at.x + rect.width() - style::PADDING_X_SM - value.width, y),
                // The rate is the only row a re-quote actually invalidates.
                if index == 0 { ink } else { chrome.ink },
                scene,
            );
        }
    }

    /// The destination row: its header, its chevron and its revealed field.
    fn paint_destination(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        wash: Color,
    ) {
        let rect = self.destination_box;
        let at = origin + rect.origin().to_vec2();
        scene.fill_rounded_rect(at, rect.size(), style::RADIUS_XL, wash);
        paint_panel_hairline(scene, at, rect.size(), style::RADIUS_XL, chrome.border);
        scene.push_clip_rounded(at, rect.size(), style::RADIUS_XL);

        let line = self.destination_line.size();
        self.destination_line.paint(
            at + Vec2::new(style::PADDING_X_SM, (SWAP_DEST_HEIGHT - line.height) / 2.0),
            chrome.dim_ink,
            scene,
        );
        draw_chevron(
            scene,
            Point::new(
                at.x + rect.width() - style::PADDING_X_SM - style::ICON_SIZE / 2.0,
                at.y + SWAP_DEST_HEIGHT / 2.0,
            ),
            style::ICON_SIZE,
            self.chevron.value().clamp(0.0, 1.0),
            chrome.dim_ink,
        );

        if rect.height() > SWAP_DEST_HEIGHT + 1.0 {
            scene.fill_rect(
                Point::new(at.x, at.y + SWAP_DEST_HEIGHT),
                Size::new(rect.width(), style::BORDER_WIDTH),
                chrome.border,
            );
            let field = Rect::from_origin_size(
                Point::new(
                    at.x + style::PADDING_X_SM,
                    at.y + SWAP_DEST_HEIGHT + style::GAP_SM,
                ),
                Size::new(
                    (rect.width() - style::PADDING_X_SM * 2.0).max(0.0),
                    (rect.height() - SWAP_DEST_HEIGHT - style::GAP_MD).max(0.0),
                ),
            );
            let has = !self.config.destination.is_empty();
            let valid = is_valid_address(&self.config.destination);
            let border = if has && !valid {
                style::with_alpha(chrome.danger_ink, 0.4)
            } else {
                chrome.border
            };
            scene.fill_rounded_rect(
                field.origin(),
                field.size(),
                style::RADIUS_LG,
                chrome.surface,
            );
            paint_panel_hairline(
                scene,
                field.origin(),
                field.size(),
                style::RADIUS_LG,
                border,
            );
            let value = self.destination_value.size();
            self.destination_value.paint(
                Point::new(
                    field.x0 + style::GAP_MD,
                    field.y0 + (field.height() - value.height) / 2.0,
                ),
                if has { chrome.ink } else { chrome.dim_ink },
                scene,
            );
            if has {
                draw_address_mark(
                    scene,
                    Point::new(
                        field.x1 - style::GAP_MD - style::ICON_SIZE / 2.0,
                        field.center().y,
                    ),
                    style::ICON_SIZE,
                    valid,
                    if valid { chrome.ink } else { chrome.danger_ink },
                );
            }
        }
        scene.pop_clip();
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
        let enabled = self.state.is_enabled();
        let scale = if enabled {
            press_scale(SWAP_TAP_SCALE, self.action_press.value())
        } else {
            1.0
        };
        let rect = self.action_box;
        let box_size = Size::new(rect.width() * scale, rect.height() * scale);
        let at = origin
            + Vec2::new(
                rect.x0 + (rect.width() - box_size.width) / 2.0,
                rect.y0 + (rect.height() - box_size.height) / 2.0,
            );
        let (fill, ink) = if enabled {
            (accent, on_accent)
        } else {
            (
                style::with_alpha(accent, SWAP_DISABLED_FILL_ALPHA),
                chrome.dim_ink,
            )
        };
        scene.fill_rounded_rect(at, box_size, SWAP_FIELD_RADIUS, fill);
        let label = self.action.size();
        self.action.paint(
            at + Vec2::new(
                (box_size.width - label.width) / 2.0,
                (box_size.height - label.height) / 2.0,
            ),
            ink,
            scene,
        );
    }

    /// The `Widget::event` pointer arm.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        if !inside(p.position, size) && p.phase != PointerPhase::Up {
            return EventResult::Ignored;
        }
        let over = self.target_at(p.position);
        match p.phase {
            PointerPhase::Move => {
                if let Some(target) = over {
                    ctx.claim_hover();
                    if target != Target::Submit || self.state.is_enabled() {
                        ctx.set_cursor(style::ACTIVE_CURSOR);
                    } else {
                        ctx.set_cursor(style::DISABLED_CURSOR);
                    }
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
                ctx.capture_pointer();
                self.armed = Some(target);
                match target {
                    Target::Flip => self.flip_press.retarget(1.0),
                    Target::Submit if self.state.is_enabled() => self.action_press.retarget(1.0),
                    Target::Token(_) | Target::Settings | Target::Max => {
                        self.affordance_press_target = Some(target);
                        self.affordance_press.retarget(1.0);
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
                self.affordance_press_target = None;
                self.flip_press.retarget(0.0);
                self.action_press.retarget(0.0);
                self.affordance_press.retarget(0.0);
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
                self.affordance_press_target = None;
                self.flip_press.retarget(0.0);
                self.action_press.retarget(0.0);
                self.affordance_press.retarget(0.0);
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

/// lucide's own viewBox extent.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// lucide's default `strokeWidth`, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

/// Paint lucide's `arrow-down-up` — the flip button's mark.
fn draw_arrows(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    path.move_to(Point::new(-5.0 * scale, -7.0 * scale));
    path.line_to(Point::new(-5.0 * scale, 7.0 * scale));
    path.move_to(Point::new(-9.0 * scale, 3.0 * scale));
    path.line_to(Point::new(-5.0 * scale, 7.0 * scale));
    path.line_to(Point::new(-scale, 3.0 * scale));
    path.move_to(Point::new(5.0 * scale, 7.0 * scale));
    path.line_to(Point::new(5.0 * scale, -7.0 * scale));
    path.move_to(Point::new(1.0 * scale, -3.0 * scale));
    path.line_to(Point::new(5.0 * scale, -7.0 * scale));
    path.line_to(Point::new(9.0 * scale, -3.0 * scale));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint lucide's `settings` — a ring with four spokes, standing in for the
/// gear's twelve teeth.
fn draw_settings(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let radius = 5.0 * scale;
    let mut path = BezPath::new();
    for (dx, dy) in [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)] {
        path.move_to(Point::new(dx * radius, dy * radius));
        path.line_to(Point::new(dx * 9.0 * scale, dy * 9.0 * scale));
    }
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
    let ring = frust::authoring::RoundedRect::new(
        centre.x - radius,
        centre.y - radius,
        centre.x + radius,
        centre.y + radius,
        radius,
    );
    scene.stroke_path(
        Point::ORIGIN,
        &frust::authoring::Shape::to_path(&ring, style::PATH_TOLERANCE),
        LUCIDE_STROKE * scale,
        &Brush::Solid(color),
    );
}

/// Paint lucide's `chevron-down`, `turn` of the way through its half-turn.
fn draw_chevron(scene: &mut dyn PaintScene, centre: Point, extent: f64, turn: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    path.move_to(Point::new(-5.0 * scale, -2.0 * scale));
    path.line_to(Point::new(0.0, 3.0 * scale));
    path.line_to(Point::new(5.0 * scale, -2.0 * scale));
    scene.push_transform(Affine::rotate_about(
        std::f64::consts::PI * turn.clamp(0.0, 1.0),
        centre,
    ));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
    scene.pop_transform();
}

/// Paint lucide's `check` or `x` — the address field's own verdict.
fn draw_address_mark(
    scene: &mut dyn PaintScene,
    centre: Point,
    extent: f64,
    valid: bool,
    color: Color,
) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    if valid {
        path.move_to(Point::new(-5.0 * scale, 0.0));
        path.line_to(Point::new(-scale, 4.0 * scale));
        path.line_to(Point::new(6.0 * scale, -5.0 * scale));
    } else {
        let arm = 5.0 * scale;
        path.move_to(Point::new(-arm, -arm));
        path.line_to(Point::new(arm, arm));
        path.move_to(Point::new(arm, -arm));
        path.line_to(Point::new(-arm, arm));
    }
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(500.0, 800.0);

    // ---- Validation and formatting -----------------------------------------

    #[test]
    fn an_address_is_valid_as_a_full_hex_word_or_a_long_enough_name() {
        assert!(is_valid_address(
            "0x1234567890abcdef1234567890abcdef12345678"
        ));
        assert!(is_valid_address(
            "0xABCDEF1234567890abcdef1234567890ABCDEF12"
        ));
        // One digit short, one too long, and one non-hex character all fail.
        assert!(!is_valid_address(
            "0x1234567890abcdef1234567890abcdef1234567"
        ));
        assert!(!is_valid_address(
            "0x1234567890abcdef1234567890abcdef123456789"
        ));
        assert!(!is_valid_address(
            "0xz234567890abcdef1234567890abcdef12345678"
        ));
        // Names need a supported suffix and more than five characters.
        assert!(is_valid_address("alice.eth"));
        assert!(is_valid_address("bob.sol"));
        assert!(!is_valid_address("a.eth"), "five characters is too short");
        assert!(!is_valid_address("alice.com"));
        assert!(!is_valid_address(""));
    }

    #[test]
    fn only_a_full_hex_address_is_truncated() {
        assert_eq!(
            truncate_address("0x1234567890abcdef1234567890abcdef12345678"),
            "0x1234...5678"
        );
        assert_eq!(truncate_address("alice.eth"), "alice.eth");
        assert_eq!(truncate_address("0xabc"), "0xabc");
    }

    #[test]
    fn a_multibyte_value_that_only_looks_like_a_full_address_in_bytes_is_never_sliced_mid_codepoint()
     {
        // 2 ASCII + twelve 3-byte codepoints + 4 ASCII = 42 *bytes*, but only
        // 18 *characters* — a crafted multibyte address that panicked
        // ("byte index 6 is not a char boundary") through the old byte-length check.
        let value = format!("0x{}{}", "日".repeat(12), ".eth");
        assert_eq!(value.len(), 42, "the crafted input is 42 bytes, not chars");
        assert_eq!(
            truncate_address(&value),
            value,
            "not 42 characters, so it must be shown as-is rather than sliced"
        );

        // Reached indirectly through the widget's own label path:
        // `is_valid_address` accepts it (it ends in `.eth`), and
        // `swap_action_label` truncates the destination — that path must not
        // panic either.
        assert!(is_valid_address(&value));
        let label = swap_action_label(SwapActionState::SendTo, &eth(), &sol(), &value);
        assert_eq!(label, format!("Swap + Send to {value}"));

        // A genuine 42-*character* multibyte address is still truncated, on a
        // char boundary rather than a byte one.
        let wide = format!("0x{}{}", "€".repeat(38), "ab");
        let chars: Vec<char> = wide.chars().collect();
        assert_eq!(chars.len(), 42);
        let head: String = chars[..6].iter().collect();
        let tail: String = chars[38..].iter().collect();
        assert_eq!(truncate_address(&wide), format!("{head}...{tail}"));
    }

    #[test]
    fn an_amount_drops_trailing_zeros_and_groups_past_a_thousand() {
        assert_eq!(format_amount(0.0, 6), "0");
        assert_eq!(format_amount(f64::NAN, 6), "0");
        assert_eq!(format_amount(1.0, 6), "1");
        assert_eq!(format_amount(1.5, 6), "1.5");
        assert_eq!(format_amount(0.123456789, 6), "0.123457");
        // Two fraction digits and grouping from 1000 up, as upstream's own
        // branch does.
        assert_eq!(format_amount(1234.5678, 6), "1,234.57");
        assert_eq!(format_amount(1_234_567.0, 6), "1,234,567");
        assert_eq!(format_amount(999.999999, 6), "999.999999");
        assert_eq!(format_amount(-2.5, 6), "-2.5");
        // A tighter cap is honoured.
        assert_eq!(format_amount(0.123456789, 2), "0.12");
    }

    // ---- The order's own state ---------------------------------------------

    fn eth() -> SwapToken {
        swap_token("ETH", "Ethereum").balance(2.0).usd(3_000.0)
    }

    fn sol() -> SwapToken {
        swap_token("SOL", "Solana").balance(50.0).usd(150.0)
    }

    #[test]
    fn the_rate_is_the_price_ratio_and_falls_back_to_parity() {
        assert_eq!(swap_rate(&eth(), &sol()), 20.0);
        assert_eq!(swap_rate(&sol(), &eth()), 0.05);
        // A token with no price quoted resolves to parity rather than to
        // infinity or zero.
        assert_eq!(swap_rate(&swap_token("A", "X"), &sol()), 1.0);
        assert_eq!(swap_rate(&eth(), &swap_token("B", "Y")), 1.0);
    }

    #[test]
    fn the_action_state_follows_the_upstream_precedence() {
        assert_eq!(
            swap_action_state(0.0, &eth(), ""),
            SwapActionState::NoAmount
        );
        assert_eq!(
            swap_action_state(-1.0, &eth(), ""),
            SwapActionState::NoAmount
        );
        // The balance check outranks a valid destination.
        assert_eq!(
            swap_action_state(5.0, &eth(), "alice.eth"),
            SwapActionState::OverBalance
        );
        assert_eq!(
            swap_action_state(1.0, &eth(), "alice.eth"),
            SwapActionState::SendTo
        );
        // An invalid destination falls through to the plain swap.
        assert_eq!(
            swap_action_state(1.0, &eth(), "nope"),
            SwapActionState::Ready
        );
        assert_eq!(swap_action_state(1.0, &eth(), ""), SwapActionState::Ready);
        // Only the two valid states may be pressed.
        assert!(!SwapActionState::NoAmount.is_enabled());
        assert!(!SwapActionState::OverBalance.is_enabled());
        assert!(SwapActionState::SendTo.is_enabled());
        assert!(SwapActionState::Ready.is_enabled());
    }

    #[test]
    fn the_action_label_names_the_state_it_is_in() {
        assert_eq!(
            swap_action_label(SwapActionState::NoAmount, &eth(), &sol(), ""),
            "Enter an amount"
        );
        assert_eq!(
            swap_action_label(SwapActionState::OverBalance, &eth(), &sol(), ""),
            "Insufficient ETH"
        );
        assert_eq!(
            swap_action_label(SwapActionState::Ready, &eth(), &sol(), ""),
            "Swap ETH → SOL"
        );
        assert_eq!(
            swap_action_label(
                SwapActionState::SendTo,
                &eth(),
                &sol(),
                "0x1234567890abcdef1234567890abcdef12345678"
            ),
            "Swap + Send to 0x1234...5678"
        );
    }

    #[test]
    fn the_two_sides_are_each_other_s_opposite() {
        assert_eq!(SwapSide::From.other(), SwapSide::To);
        assert_eq!(SwapSide::To.other(), SwapSide::From);
        assert_eq!(SwapSide::From.label(), "You pay");
        assert_eq!(SwapSide::To.label(), "You get");
    }

    // ---- The mounted widget -------------------------------------------------

    #[derive(Default)]
    struct App {
        flips: u32,
        picks: Vec<SwapSide>,
        amounts: Vec<f64>,
        toggles: Vec<bool>,
        submits: u32,
        amount: f64,
        destination: String,
        reversed: bool,
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
                    amount: 1.0,
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
                let (from, to) = if s.reversed {
                    (sol(), eth())
                } else {
                    (eth(), sol())
                };
                let widget = swap(from, to)
                    .amount(s.amount)
                    .destination(s.destination.clone())
                    .on_flip(|s: &mut App| {
                        s.flips += 1;
                        s.reversed = !s.reversed;
                    })
                    .on_pick_token(|s: &mut App, side| s.picks.push(side))
                    .on_amount_change(|s: &mut App, amount| {
                        s.amounts.push(amount);
                        s.amount = amount;
                    })
                    .on_destination_toggle(|s: &mut App, show| s.toggles.push(show))
                    .on_submit(|s: &mut App| s.submits += 1);
                frust::Stack(vec![any(widget)])
            };
            self.root.rebuild(&mut logic, &mut self.state);
            self.root
                .layout_with_text(WINDOW, &mut self.tcx as &mut dyn Any);
            let mut rec = Recorder::default();
            let now = self.clock;
            self.root.paint(&mut rec, ft_ms(now));
            rec
        }

        /// Paint one frame with **no** relayout in between — the shape a bare
        /// `request_frame`/`request_redraw` actually drives in production,
        /// unlike [`Self::step`], which relayouts unconditionally every call
        /// (masking a missed `request_layout`, per docs/REVIEW_FOCUS.md's
        /// layout-skip hot spot). Returns whether the paint itself asked for
        /// a relayout, so a missed call fails the assertion instead of being
        /// silently absorbed by an intervening layout pass.
        fn paint_only(&mut self, ms: f64) -> (Recorder, bool) {
            self.clock += ms;
            let mut rec = Recorder::default();
            let outcome = self.root.paint(&mut rec, ft_ms(self.clock));
            (rec, outcome.needs_layout)
        }

        fn event(&mut self, event: InputEvent) {
            self.root.event(&mut self.state, &event);
        }

        fn click(&mut self, at: Point) {
            self.event(pointer(PointerPhase::Down, at.x, at.y));
            self.event(pointer(PointerPhase::Up, at.x, at.y));
            self.step(0.0);
        }

        /// The flip button's centre, in window space.
        fn flip_centre(&mut self) -> Point {
            let inner = SWAP_WIDTH - SWAP_PADDING * 2.0;
            let seam = SWAP_HEADER_HEIGHT + SWAP_PADDING + SWAP_FIELD_HEIGHT + SWAP_GAP / 2.0;
            Point::new(SWAP_PADDING + inner / 2.0, seam)
        }

        /// The token affordance's centre for `side`.
        fn token_centre(&mut self, side: SwapSide) -> Point {
            let index = if side == SwapSide::From { 0 } else { 1 };
            let y = SWAP_HEADER_HEIGHT
                + SWAP_PADDING
                + index as f64 * (SWAP_FIELD_HEIGHT + SWAP_GAP)
                + SWAP_FIELD_PADDING
                + style::GAP_MD
                + SWAP_TOKEN_HEIGHT / 2.0;
            Point::new(SWAP_WIDTH - SWAP_PADDING - SWAP_FIELD_PADDING - 20.0, y)
        }

        /// The destination row header's centre.
        fn destination_centre(&mut self) -> Point {
            let rec = self.step(0.0);
            // The destination row is the last `RADIUS_XL` box the card paints.
            let row = rec
                .rrects
                .iter()
                .rev()
                .find(|(_, _, radius, _)| (*radius - style::RADIUS_XL).abs() < 0.001)
                .copied()
                .expect("the destination row");
            Point::new(
                row.0.x + row.1.width / 2.0,
                row.0.y + SWAP_DEST_HEIGHT / 2.0,
            )
        }

        /// The action button's centre.
        fn action_centre(&mut self) -> Point {
            let rec = self.step(0.0);
            let button = rec
                .rrects
                .iter()
                .rev()
                .find(|(_, size, _, _)| (size.height - SWAP_ACTION_HEIGHT).abs() < 0.001)
                .copied()
                .expect("the action button");
            Point::new(
                button.0.x + button.1.width / 2.0,
                button.0.y + button.1.height / 2.0,
            )
        }
    }

    #[test]
    fn the_flip_button_accumulates_its_turn_rather_than_unwinding() {
        let mut h = Harness::new();
        let at = h.flip_centre();
        h.click(at);
        assert_eq!(h.state.flips, 1);
        assert!(h.state.reversed, "the app swapped the pair");
        h.click(at);
        assert_eq!(h.state.flips, 2);
        assert!(!h.state.reversed);
        // Two presses is a full turn, not back to where it started.
        assert_eq!(SWAP_FLIP_TURN * 2.0, 360.0);
    }

    #[test]
    fn each_token_affordance_reports_its_own_side() {
        let mut h = Harness::new();
        let from = h.token_centre(SwapSide::From);
        h.click(from);
        let to = h.token_centre(SwapSide::To);
        h.click(to);
        assert_eq!(h.state.picks, vec![SwapSide::From, SwapSide::To]);
    }

    #[test]
    fn max_reports_the_pay_token_s_whole_balance() {
        let mut h = Harness::new();
        // `Max` sits on the pay field's balance line, at its right edge.
        let inner = SWAP_WIDTH - SWAP_PADDING * 2.0;
        let at = Point::new(
            SWAP_PADDING + inner - SWAP_FIELD_PADDING - 12.0,
            SWAP_HEADER_HEIGHT + SWAP_PADDING + SWAP_FIELD_HEIGHT - SWAP_FIELD_PADDING - 6.0,
        );
        h.click(at);
        assert_eq!(h.state.amounts, vec![2.0], "ETH's own balance");
    }

    #[test]
    fn the_destination_row_toggles_and_reports_once_per_press() {
        let mut h = Harness::new();
        let at = h.destination_centre();
        h.click(at);
        assert_eq!(h.state.toggles, vec![true]);
        let at = h.destination_centre();
        h.click(at);
        assert_eq!(h.state.toggles, vec![true, false]);
    }

    #[test]
    fn opening_the_destination_row_makes_the_card_taller() {
        let mut h = Harness::new();
        let closed = h.step(0.0).rrects[0].1.height;
        let at = h.destination_centre();
        h.click(at);
        h.step(1_000.0);
        let open = h.step(0.0).rrects[0].1.height;
        assert!(open > closed, "the reveal did not grow the card");
    }

    #[test]
    fn the_action_button_only_fires_when_the_order_is_valid() {
        let mut h = Harness::new();
        h.state.amount = 0.0;
        h.step(0.0);
        let at = h.action_centre();
        h.click(at);
        assert_eq!(h.state.submits, 0, "an empty order is not submittable");

        h.state.amount = 1.0;
        h.step(0.0);
        let at = h.action_centre();
        h.click(at);
        assert_eq!(h.state.submits, 1);

        // Over the balance, it stops again.
        h.state.amount = 99.0;
        h.step(0.0);
        let at = h.action_centre();
        h.click(at);
        assert_eq!(h.state.submits, 1);
    }

    #[test]
    fn a_new_amount_dims_the_get_side_and_then_settles() {
        let mut h = Harness::new();
        h.state.amount = 2.0;
        // The rebuild arms the re-quote; the paint that follows latches its
        // clock, so the dim is visible from that frame.
        let quoting = h.step(0.0);
        let settled = h.step(SWAP_QUOTE_SETTLE.as_secs_f64() * 1_000.0 + 10.0);
        assert_ne!(
            quoting.inks, settled.inks,
            "the get side never dimmed while re-quoting"
        );
        // ...and it stays settled rather than re-arming itself.
        let after = h.step(16.0);
        assert_eq!(settled.inks, after.inks);
    }

    #[test]
    fn a_cancelled_press_never_reaches_the_app() {
        let mut h = Harness::new();
        let at = h.flip_centre();
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Cancel, at.x, at.y));
        assert_eq!(h.state.flips, 0);
        // ...and neither does one released somewhere else.
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y + 200.0));
        assert_eq!(h.state.flips, 0);
    }

    #[test]
    fn a_reduced_motion_widget_places_its_flip_and_reveal_at_once() {
        let mut h = Harness::themed(reduced());
        let at = h.flip_centre();
        h.click(at);
        let first = h.step(0.0);
        let second = h.step(16.0);
        assert_eq!(
            first.rrects, second.rrects,
            "a reduced-motion flip was still turning"
        );
    }

    // ---- The destination reveal's relayout coverage ----

    #[test]
    fn a_reduced_motion_destination_toggle_relayouts_on_the_next_paint_only_frame() {
        let mut h = Harness::themed(reduced());
        let closed_height = h.step(0.0).rrects[0].1.height;
        let at = h.destination_centre();
        // `Target::Destination`'s own handler only requests a redraw
        // (`fire`'s trailing `ctx.request_redraw()`); the relayout must come
        // from the very next paint, not from the event itself — no `step`
        // (which always relayouts) runs in between.
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));
        let (_, needs_layout) = h.paint_only(0.0);
        assert!(
            needs_layout,
            "the reduce_motion destination snap never asked for a relayout"
        );
        let open_height = h.step(0.0).rrects[0].1.height;
        assert!(
            open_height > closed_height,
            "the destination row never grew under reduce_motion: {closed_height} -> {open_height}"
        );
    }

    #[test]
    fn the_reveal_s_settling_frame_still_requests_layout() {
        let mut h = Harness::new();
        let at = h.destination_centre();
        h.event(pointer(PointerPhase::Down, at.x, at.y));
        h.event(pointer(PointerPhase::Up, at.x, at.y));

        // The first paint after a retarget latches the animation's start
        // time, so it always reads as elapsed-zero regardless of the delta
        // passed in — the `tool_result.rs` reveal carries the same rule.
        let (_, opening) = h.paint_only(0.0);
        assert!(opening, "the reveal's opening frame skipped its relayout");

        // Mid-flight, well short of `SWAP_DEST_REVEAL`.
        let (_, mid) = h.paint_only(SWAP_DEST_REVEAL.as_secs_f64() * 1_000.0 / 2.0);
        assert!(mid, "a mid-flight reveal frame must relayout too");

        // The exact frame the ramp settles: `Lane::advance` returns `false`
        // here (the bug this test guards), but the value still moves from
        // its mid-flight sample to the target, which the before/after
        // compare must still catch.
        let (_, settling) = h.paint_only(SWAP_DEST_REVEAL.as_secs_f64() * 1_000.0 / 2.0 + 10.0);
        assert!(
            settling,
            "the reveal's settling frame skipped its relayout — the last-laid \
             geometry sits a hair short of rest"
        );

        // One frame later, at rest: nothing left to relayout for.
        let (_, resting) = h.paint_only(16.0);
        assert!(!resting, "a resting reveal keeps asking for a relayout");
    }

    // ---- The token/settings/`Max` press feedback ----

    fn build_swap() -> SwapWidget {
        let mut counter = 0u64;
        let view: SwapView<()> = swap(eth(), sol()).amount(1.0);
        View::<()>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn layout_swap(w: &mut SwapWidget) -> Size {
        let mut tcx = TextContext::new();
        let mut ctx = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        w.layout(&mut ctx, &BoxConstraints::new(Size::ZERO, WINDOW))
    }

    /// Dispatch a pointer `phase` at `at` (widget-local) directly to `w`,
    /// bypassing `RenderRoot` — the same seam `tool_result.rs`'s own
    /// `paint_at` uses for its `PaintCtx`, extended to `EventCtx`.
    fn press_swap(w: &mut SwapWidget, size: Size, at: Point, phase: PointerPhase) {
        let mut state = ();
        let mut ctx = EventCtx::new(&mut state as &mut dyn Any, Point::ZERO, size);
        w.event(&mut ctx, &pointer(phase, at.x, at.y));
    }

    fn paint_swap(w: &mut SwapWidget, size: Size, ms: f64) {
        let mut rec = Recorder::default();
        let mut ctx = PaintCtx::for_test(Point::ZERO, size, ft_ms(ms));
        w.paint(&mut ctx, &mut rec);
    }

    #[test]
    fn a_pressed_token_affordance_arms_and_shrinks_only_its_own_side() {
        let mut w = build_swap();
        let size = layout_swap(&mut w);
        let at = w.tokens[0].center();

        press_swap(&mut w, size, at, PointerPhase::Down);
        assert_eq!(
            w.affordance_press_target,
            Some(Target::Token(SwapSide::From)),
            "pressing the From token affordance never armed its press lane"
        );

        // The press spring's own first paint after the retarget latches its
        // start time and so reads elapsed-zero (the same rule the reveal's
        // settling-frame tests above document); a second paint is needed to
        // see it in flight.
        paint_swap(&mut w, size, 0.0);
        paint_swap(&mut w, size, 60.0);
        let from_scale = w.affordance_scale(Target::Token(SwapSide::From));
        assert!(
            from_scale < 1.0,
            "SWAP_TAP_SCALE's documented token press shrink never took hold: {from_scale}"
        );
        assert_eq!(
            w.affordance_scale(Target::Token(SwapSide::To)),
            1.0,
            "the To side must not shrink from the From side's own press"
        );

        press_swap(&mut w, size, at, PointerPhase::Up);
        paint_swap(&mut w, size, 800.0);
        paint_swap(&mut w, size, 1_600.0);
        assert_eq!(
            w.affordance_scale(Target::Token(SwapSide::From)),
            1.0,
            "the token press shrink never released back to resting scale"
        );
    }

    #[test]
    fn the_settings_and_max_affordances_shrink_on_their_own_press() {
        let mut w = build_swap();
        let size = layout_swap(&mut w);
        assert!(
            w.max_box.width() > 0.0,
            "eth() carries a balance, so Max must be present to press"
        );

        let settings_at = w.settings.center();
        press_swap(&mut w, size, settings_at, PointerPhase::Down);
        assert_eq!(w.affordance_press_target, Some(Target::Settings));
        paint_swap(&mut w, size, 0.0);
        paint_swap(&mut w, size, 60.0);
        assert!(
            w.affordance_scale(Target::Settings) < 1.0,
            "the settings affordance never shrank on press"
        );
        press_swap(&mut w, size, settings_at, PointerPhase::Up);
        paint_swap(&mut w, size, 800.0);
        paint_swap(&mut w, size, 1_600.0);
        assert_eq!(
            w.affordance_scale(Target::Settings),
            1.0,
            "settings' press shrink never released back to resting scale"
        );

        let max_at = w.max_box.center();
        press_swap(&mut w, size, max_at, PointerPhase::Down);
        assert_eq!(w.affordance_press_target, Some(Target::Max));
        paint_swap(&mut w, size, 1_600.0);
        paint_swap(&mut w, size, 1_660.0);
        assert!(
            w.affordance_scale(Target::Max) < 1.0,
            "Max never shrank on press"
        );
        press_swap(&mut w, size, max_at, PointerPhase::Up);
        paint_swap(&mut w, size, 2_400.0);
        paint_swap(&mut w, size, 3_200.0);
        assert_eq!(
            w.affordance_scale(Target::Max),
            1.0,
            "Max's press shrink never released back to resting scale"
        );
    }

    // ---- Typeface: every run of the swap card follows the live theme -------

    use crate::text::typeface_probe::{
        Probe, assert_follows_a_live_family_swap, assert_paints_only_in_geist,
    };

    /// A funded swap with an amount entered, so both sides, the quote rows and
    /// the action all carry text.
    fn probe_view(_: &mut ()) -> frust::StackView<()> {
        frust::Stack(vec![any(swap::<()>(
            swap_token("ETH", "Ethereum").balance(2.0).usd(3_000.0),
            swap_token("USDC", "Ethereum").balance(10.0).usd(1.0),
        )
        .amount(0.5))])
    }

    #[test]
    fn swap_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the swap card's text", probe_view, WINDOW);
        let runs = Probe::new(probe_view, WINDOW, crate::theme()).frame();
        assert_eq!(
            runs.len(),
            22,
            "the title, Max, the destination line and the action, five runs per \
             side, and four quote rows of two"
        );
    }

    #[test]
    fn swap_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the swap card's text", probe_view, WINDOW);
    }
}
