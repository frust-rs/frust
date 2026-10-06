//! Ports beUI's `wallet-card` block — `components/motion/wallet-card/*` (beUI
//! rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01),
//! registry slug `wallet-card`: *"Wallet overview card with an account switcher
//! and search that morph open from their triggers, a cascading balance with a
//! live change pill and privacy toggle, copy-address, and Send / Deposit / Swap
//! / Buy actions."*
//!
//! Upstream ships eight files behind the slug (`index`, `account-switcher`,
//! `search-bar`, `balance-delta`, `actions`, `copy-button`, `account-avatar`,
//! plus `constants`/`types`/`utils`); all of them are carried here as one
//! widget, because every one of those parts is positioned against the same
//! header row and the same morph origin.
//!
//! # A premise correction
//!
//! The porting card describes this slug as *"stacked wallet cards that fan/slide
//! on selection (depth stack springs, selected card lifts)"*. Upstream has no
//! card stack at all: it is a **single** wallet card whose *account switcher*
//! and *search* morph open from their own triggers into panels that span the
//! header row. The port follows upstream. The fanning card deck the card
//! describes is a real upstream component — it is `not-found`'s `stacked`
//! style — and is ported in [`crate::blocks::not_found`].
//!
//! | upstream | here |
//! |---|---|
//! | `max-w-xs rounded-4xl border p-6` | [`WALLET_WIDTH`], [`WALLET_RADIUS`], [`WALLET_PADDING`] |
//! | `MORPH` `{spring, duration 0.5, bounce 0.22}` | [`WALLET_MORPH`] |
//! | `LIST`/`ITEM` (`staggerChildren 0.035`, `delayChildren 0.12`, `y: -6`) | [`WALLET_ITEM_STAGGER`], [`WALLET_LIST_DELAY`], [`WALLET_ITEM_TRAVEL`] |
//! | `HEAD` `px-2 py-1.5 gap-2` trigger/panel-header padding | [`WALLET_HEAD_PADDING_X`], [`WALLET_HEAD_PADDING_Y`] |
//! | `AccountAvatar`'s `h-7 w-7 rounded-full` | [`WALLET_AVATAR_SIZE`] |
//! | `truncateAddress` | [`truncate_address`] |
//! | `ActionSwapText` balance / `"*".repeat(7)` mask | [`WALLET_MASK`], cascaded per grapheme |
//! | `BalanceDelta`'s tinted pill and trend arrow | [`format_delta`], [`WALletDeltaTone`](WalletDeltaTone) |
//! | `WalletActions`' four `h-12 w-12 rounded-full` buttons on `SPRING_PRESS` | [`WalletAction`], [`WALLET_ACTION_SIZE`] |
//! | `hasNotifications`' `animate-ping` dot | [`WALLET_PULSE_PERIOD`], [`wallet_pulse`] |
//!
//! # Degradations against the web original
//!
//! - **The search panel is a picker, not a text field.** Upstream's morphing bar
//!   holds an `<input>` that filters the recent list as you type. A controlled
//!   text field is [`crate::components::input`], and threading its value through
//!   this card would put an app-owned query string in the middle of a card whose
//!   every other input is a press; the panel here lists the recent terms and
//!   reports the one picked through
//!   [`WalletCardView::on_search_submit`]. A caller who needs live filtering
//!   composes `input` above the card and hands the filtered list back in.
//! - **No avatars.** `AccountAvatar` falls back to a remote DiceBear SVG; there
//!   is no image fetch at this tier, so an account's disc is a tinted circle
//!   carrying its initial.
//! - **The copy button reports, it does not copy.** There is no clipboard seam
//!   in the facade this crate is limited to, so
//!   [`WalletCardView::on_copy_address`] hands the address to the app and the
//!   icon still swaps to its check for [`WALLET_COPIED_HOLD`].
//! - **No backdrop blur**, the catalog-wide absence.
//!
//! # The true-3D account fan, behind `gpu-effects`
//!
//! With the non-default `gpu-effects` feature on, [`WalletCardView::gpu_fan`]
//! renders the open account switcher's rows as a genuine **depth-fanned
//! stack**: one perspective-projected plate per account, the selected one
//! lifted toward the viewer and its neighbours receding and leaning away in a
//! curve, occluding each other by *distance* rather than by paint order
//! ([`fan_scene`](crate::gpu_fx::card3d::fan_scene)).
//!
//! It stands where the porting card asked for a fanning card deck. Upstream
//! ships none — see *A premise correction* above — and the account switcher's
//! morph is the one place this block has a stack of cards at all: closed, the
//! rows are collapsed behind the trigger; open, they are a run of card
//! surfaces the selected one is picked out of. So that run is what the 3D
//! path fans.
//!
//! The plates are the *surfaces* only. Every row's avatar, name, address and
//! copy affordance keeps painting on the 2D path above the composite,
//! unchanged, because a 3D face carries no widget content and no text — the
//! boundary the whole substrate is under. Two consequences follow:
//!
//! - **The 2D selected-row plate stands down while the fan is live**, so the
//!   selection is read off the lifted card rather than off two plates stacked
//!   on each other.
//! - **A row's stagger reveal rides on its plate's own alpha**, and a
//!   translucent face writes depth like an opaque one, so mid-stagger a plate
//!   behind another is rejected rather than showing through it. Over the
//!   third of a second the list takes to arrive that is invisible; it is
//!   recorded because it is the depth/blending interaction the substrate
//!   warns about, not an accident.
//!
//! The runtime fallback is the ordinary contract: with the GPU unreachable —
//! before a shell's first surface, on every platform whose shell installs no
//! device, under the substrate's kill switch — the panel paints exactly the
//! 2D rows it always did, `gpu_fan` set or not.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    ErasedCallback, EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx,
    PaintScene, Point, PointerEvent, PointerPhase, Rect, Role, RoundedRect, SemanticsCtx, Shape,
    Size, TickClass, Vec2, View, Widget, erase_callback, erase_callback_arg, text::TextStyle,
};
use frust::{FrameTime, Theme};

use crate::components::popover::{PanelChrome, morph_rect, paint_panel_hairline, resolve_panel};
#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::card3d::{self, Card3d, FanCard};
#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::quad3d::QuadFace;
#[cfg(feature = "gpu-effects")]
use crate::gpu_fx::schedule::request_frame;
use crate::motion::stagger::StaggerDirection;
use crate::motion::{Presence, Ramp, Stagger};
use crate::press::{Lane, inside, is_activation_key, press_scale, presses};
use crate::style;
use crate::text::{LabelRun, ThemeTextType, themed_style};
use crate::tokens::motion::{EASE_OUT, SPRING_PANEL, SPRING_PRESS};

/// The label this block's GPU pass is diagnosed under.
#[cfg(feature = "gpu-effects")]
const FX_LABEL: &str = "wallet-card";

// ---- Metrics ---------------------------------------------------------------

/// `max-w-xs` — the card's width, in logical px.
pub const WALLET_WIDTH: f64 = 320.0;

/// `rounded-4xl` — the card's corner radius.
pub const WALLET_RADIUS: f64 = style::RADIUS_4XL;

/// `p-6` — the card's own padding, in logical px.
pub const WALLET_PADDING: f64 = 24.0;

/// `h-7 w-7` — an account avatar's box, in logical px.
pub const WALLET_AVATAR_SIZE: f64 = 28.0;

/// `px-2` — the account trigger's and panel header's horizontal padding.
pub const WALLET_HEAD_PADDING_X: f64 = 8.0;

/// `py-1.5` — the same pair's vertical padding.
pub const WALLET_HEAD_PADDING_Y: f64 = 6.0;

/// `gap-2` — the gap inside the trigger and each account row.
pub const WALLET_HEAD_GAP: f64 = 8.0;

/// The header row's height: an avatar plus its padding.
pub const WALLET_HEAD_HEIGHT: f64 = WALLET_AVATAR_SIZE + WALLET_HEAD_PADDING_Y * 2.0;

/// `h-8 w-8` — the search and notification icon buttons, in logical px.
pub const WALLET_ICON_BUTTON: f64 = 32.0;

/// `rounded-lg` — their corner radius, and the search trigger's morph origin.
pub const WALLET_ICON_RADIUS: f64 = style::RADIUS_LG;

/// `borderRadius: 16` — the radius both morphing panels resolve to.
pub const WALLET_PANEL_RADIUS: f64 = style::RADIUS_2XL;

/// `mt-8` — the gap between the header row and the balance block.
pub const WALLET_BLOCK_GAP: f64 = 32.0;

/// `text-3xl` — the balance's type size, in logical px.
pub const WALLET_BALANCE_SIZE: f64 = 30.0;

/// `h-7` — the delta pill's row height, in logical px.
pub const WALLET_DELTA_HEIGHT: f64 = 28.0;

/// `h-12 w-12` — one action's disc, in logical px.
pub const WALLET_ACTION_SIZE: f64 = 48.0;

/// `gap-2` — the gap between an action's disc and its label.
pub const WALLET_ACTION_GAP: f64 = 8.0;

/// One account row's height inside the switcher panel, in logical px.
pub const WALLET_ROW_HEIGHT: f64 = 48.0;

/// `max-h-64` — the switcher panel's own list cap, in logical px.
pub const WALLET_LIST_MAX_HEIGHT: f64 = 256.0;

/// `p-1.5` — the padding around either panel's list.
pub const WALLET_LIST_PADDING: f64 = 6.0;

/// `"*".repeat(7)` — what a hidden balance shows instead.
pub const WALLET_MASK: &str = "*******";

/// `bg-emerald-500/15` and `bg-red-500/15` — the delta pill's wash alpha.
pub const WALLET_DELTA_WASH_ALPHA: f32 = 0.15;

/// How long the copy affordance holds its check before swapping back
/// (upstream's `setTimeout(…, 1400)`).
pub const WALLET_COPIED_HOLD: Duration = Duration::from_millis(1_400);

// ---- Motion ----------------------------------------------------------------

/// `MORPH` — the spring the trigger grows into its panel on.
///
/// Upstream authors it as `{ type: "spring", duration: 0.5, bounce: 0.22 }`,
/// which is Motion's duration/bounce parameterisation rather than the
/// mass/stiffness/damping triple [`crate::tokens::motion`]'s constants use.
/// `SPRING_PANEL` is the catalog's own panel spring and is what the rest of the
/// overlay family morphs on, so it is used here rather than a ninth
/// one-component spring — the visible difference over a 0.5s morph is the
/// bounce, and `SPRING_PANEL` is the near-critically-damped member of the same
/// family.
pub const WALLET_MORPH: Ramp = Ramp::spring(SPRING_PANEL);

/// `staggerChildren: 0.035` — the gap between account rows revealing.
pub const WALLET_ITEM_STAGGER: Duration = Duration::from_millis(35);

/// `delayChildren: 0.12` — how long the list waits before its first row moves.
pub const WALLET_LIST_DELAY: Duration = Duration::from_millis(120);

/// `ITEM`'s `y: -6` — how far above its slot a revealing row starts, in logical
/// px.
pub const WALLET_ITEM_TRAVEL: f64 = -6.0;

/// `whileTap={{ scale: 0.94 }}` — an action's press shrink.
pub const WALLET_PRESS_SCALE: f64 = 0.94;

/// `animate-ping`'s cycle — how long the unread dot's halo takes to expand and
/// fade.
pub const WALLET_PULSE_PERIOD: Duration = Duration::from_millis(1_000);

/// How far past the dot the unread halo expands, as a multiple of its radius.
pub const WALLET_PULSE_REACH: f64 = 2.0;

/// How opaque an unselected card's plate is in the true-3D fan, relative to
/// the selected one's.
///
/// The 2D panel paints a plate under the *selected* row only, so the fan needs
/// a value for the rest: dim enough that the lifted card still reads as the
/// selection, solid enough that the stack reads as a stack rather than as one
/// card floating over nothing.
#[cfg(feature = "gpu-effects")]
pub const WALLET_FAN_PLATE_ALPHA: f32 = 0.45;

/// The unread halo's scale and alpha at `elapsed` — `animate-ping`'s own shape:
/// it grows from the dot to [`WALLET_PULSE_REACH`] times its size while fading
/// to nothing, then restarts.
///
/// A zero-length period is a still dot rather than a division by zero.
pub fn wallet_pulse(elapsed: Duration) -> (f64, f64) {
    if WALLET_PULSE_PERIOD.is_zero() {
        return (1.0, 0.0);
    }
    let phase = (elapsed.as_nanos() % WALLET_PULSE_PERIOD.as_nanos()) as f64
        / WALLET_PULSE_PERIOD.as_nanos() as f64;
    (1.0 + (WALLET_PULSE_REACH - 1.0) * phase, 1.0 - phase)
}

// ---- Formatting ------------------------------------------------------------

/// `truncateAddress`: `0x1234…cdef` for anything over twelve characters, the
/// address itself otherwise.
///
/// Counted in **characters, not bytes**, so a non-ASCII address label is cut on
/// a grapheme rather than mid-codepoint.
pub fn truncate_address(address: &str) -> String {
    let chars: Vec<char> = address.chars().collect();
    if chars.len() <= 12 {
        return address.to_string();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// The balance line: `prefix` then the amount to two decimals, thousands
/// grouped — upstream's `toLocaleString` with both fraction digits pinned.
///
/// The separator is an ASCII comma rather than a locale-resolved one, for the
/// reason [`crate::components::number`] records about its own grouping: the
/// facade publishes no locale seam.
pub fn format_balance(amount: f64, prefix: &str) -> String {
    format!("{prefix}{}", group_fixed(amount.abs(), amount < 0.0))
}

/// The delta pill's line: a sign, a currency mark and the magnitude.
pub fn format_delta(delta: f64) -> String {
    let sign = if delta >= 0.0 { '+' } else { '-' };
    format!("{sign}${}", group_fixed(delta.abs(), false))
}

/// `value` at two decimals with thousands grouped, optionally negated.
fn group_fixed(value: f64, negative: bool) -> String {
    let fixed = format!("{value:.2}");
    let (whole, fraction) = fixed.split_once('.').unwrap_or((fixed.as_str(), "00"));
    let mut grouped = String::new();
    for (index, ch) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if negative {
        format!("-{grouped}.{fraction}")
    } else {
        format!("{grouped}.{fraction}")
    }
}

/// Which way the balance last moved — what tints the delta pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletDeltaTone {
    /// The balance went up: the emerald pill and the rising arrow.
    Up,
    /// It went down: the red pill and the falling arrow.
    Down,
}

impl WalletDeltaTone {
    /// The tone a `delta` reports — upstream's `(delta?.amount ?? 0) > 0`, so a
    /// zero change reads as a fall rather than as nothing.
    pub fn of(delta: f64) -> Self {
        if delta > 0.0 {
            WalletDeltaTone::Up
        } else {
            WalletDeltaTone::Down
        }
    }
}

// ---- Accounts and actions --------------------------------------------------

/// One switchable account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletAccount {
    id: String,
    name: String,
    address: String,
}

/// An account with `id`, shown as `name` and holding `address`.
pub fn wallet_account(
    id: impl Into<String>,
    name: impl Into<String>,
    address: impl Into<String>,
) -> WalletAccount {
    WalletAccount {
        id: id.into(),
        name: name.into(),
        address: address.into(),
    }
}

impl WalletAccount {
    /// The account's stable id — what [`WalletCardView::on_account_change`]
    /// reports.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Its display name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its address, in full.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The disc's stand-in for an avatar: the name's first character, upper
    /// cased. Taken over the whole first grapheme rather than by byte, so an
    /// emoji or astral name renders its own glyph — upstream's own `initials`
    /// note about surrogate pairs.
    pub fn initial(&self) -> String {
        self.name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_default()
    }
}

/// One of the four primary wallet actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletAction {
    /// `Send`.
    Send,
    /// `Deposit`.
    Deposit,
    /// `Swap`.
    Swap,
    /// `Buy`.
    Buy,
}

impl WalletAction {
    /// All four, in upstream's own row order.
    pub const ALL: [WalletAction; 4] = [
        WalletAction::Send,
        WalletAction::Deposit,
        WalletAction::Swap,
        WalletAction::Buy,
    ];

    /// The action's default label.
    pub const fn label(self) -> &'static str {
        match self {
            WalletAction::Send => "Send",
            WalletAction::Deposit => "Deposit",
            WalletAction::Swap => "Swap",
            WalletAction::Buy => "Buy",
        }
    }
}

/// Which panel the card's header row is showing, if any — the two are mutually
/// exclusive because both morph into the same box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalletPanel {
    /// Neither panel: the trigger row itself.
    #[default]
    None,
    /// The account switcher.
    Accounts,
    /// The recent-search picker.
    Search,
}

// ---- The component ---------------------------------------------------------

/// A view-held string callback (erased on build).
type OnString<State> = Rc<dyn Fn(&mut State, String)>;

/// A view-held action callback (erased on build).
type OnAction<State> = Rc<dyn Fn(&mut State, WalletAction)>;

/// What the card renders from, beyond its accounts.
#[derive(Clone, Debug, PartialEq)]
struct WalletConfig {
    account_id: Option<String>,
    balance: f64,
    balance_prefix: String,
    change: Option<f64>,
    balance_hidden: bool,
    has_notifications: bool,
    search_recent: Vec<String>,
    search_empty_label: String,
}

/// A declarative beUI wallet card. See [`wallet_card`].
pub struct WalletCardView<State: 'static> {
    accounts: Vec<WalletAccount>,
    config: WalletConfig,
    /// Whether the open account switcher fans its rows in real depth. See the
    /// [module docs](self)' *The true-3D account fan*.
    #[cfg(feature = "gpu-effects")]
    gpu_fan: bool,
    on_account_change: OnString<State>,
    on_action: OnAction<State>,
    on_search_submit: OnString<State>,
    on_copy_address: OnString<State>,
    on_notifications: Rc<dyn Fn(&mut State)>,
}

/// Build a wallet card over `accounts`, showing `balance`.
pub fn wallet_card<State: 'static>(
    accounts: Vec<WalletAccount>,
    balance: f64,
) -> WalletCardView<State> {
    WalletCardView {
        accounts,
        config: WalletConfig {
            account_id: None,
            balance,
            balance_prefix: "$".to_string(),
            change: None,
            balance_hidden: false,
            has_notifications: false,
            search_recent: Vec::new(),
            search_empty_label: "No recent searches".to_string(),
        },
        #[cfg(feature = "gpu-effects")]
        gpu_fan: false,
        on_account_change: Rc::new(|_, _| {}),
        on_action: Rc::new(|_, _| {}),
        on_search_submit: Rc::new(|_, _| {}),
        on_copy_address: Rc::new(|_, _| {}),
        on_notifications: Rc::new(|_| {}),
    }
}

impl<State: 'static> WalletCardView<State> {
    /// Take the selected account over (`accountId`).
    pub fn account_id(mut self, id: impl Into<String>) -> Self {
        self.config.account_id = Some(id.into());
        self
    }

    /// Set the currency mark the balance carries (`balancePrefix`).
    pub fn balance_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.balance_prefix = prefix.into();
        self
    }

    /// Show a change pill under the balance (`defaultChange`).
    pub fn change(mut self, change: f64) -> Self {
        self.config.change = Some(change);
        self
    }

    /// Start with the balance masked (`defaultBalanceHidden`).
    pub fn balance_hidden(mut self, hidden: bool) -> Self {
        self.config.balance_hidden = hidden;
        self
    }

    /// Show the unread pulse on the bell (`hasNotifications`).
    pub fn has_notifications(mut self, has: bool) -> Self {
        self.config.has_notifications = has;
        self
    }

    /// Set the terms the search panel offers (`searchRecent`).
    pub fn search_recent(mut self, recent: Vec<String>) -> Self {
        self.config.search_recent = recent;
        self
    }

    /// Set the line the search panel shows when it has nothing to offer.
    pub fn search_empty_label(mut self, label: impl Into<String>) -> Self {
        self.config.search_empty_label = label.into();
        self
    }

    /// Set the account-selection callback (`onAccountChange`).
    pub fn on_account_change<F: Fn(&mut State, String) + 'static>(mut self, f: F) -> Self {
        self.on_account_change = Rc::new(f);
        self
    }

    /// Set the callback the four primary actions report through.
    pub fn on_action<F: Fn(&mut State, WalletAction) + 'static>(mut self, f: F) -> Self {
        self.on_action = Rc::new(f);
        self
    }

    /// Set the callback a picked search term reports through (`onSearchSubmit`).
    pub fn on_search_submit<F: Fn(&mut State, String) + 'static>(mut self, f: F) -> Self {
        self.on_search_submit = Rc::new(f);
        self
    }

    /// Set the callback the copy affordance reports through — the card does not
    /// reach a clipboard itself (see the [module docs](self)).
    pub fn on_copy_address<F: Fn(&mut State, String) + 'static>(mut self, f: F) -> Self {
        self.on_copy_address = Rc::new(f);
        self
    }

    /// Set the bell's callback (`onNotifications`).
    pub fn on_notifications<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_notifications = Rc::new(f);
        self
    }

    /// Fan the open account switcher's rows in real depth through the true-3D
    /// path.
    ///
    /// **Opt-in, and it replaces the rows' *surfaces* only** — their avatars,
    /// names, addresses and copy affordances keep painting on the 2D path
    /// above the composite. Read the [module docs](self)' *The true-3D account
    /// fan* before reaching for it. With the GPU unreachable the panel renders
    /// exactly its 2D rows, so a caller never has to branch on availability.
    #[cfg(feature = "gpu-effects")]
    pub fn gpu_fan(mut self, fan: bool) -> Self {
        self.gpu_fan = fan;
        self
    }
}

/// One retained account row: its shaped runs and its own id.
struct AccountRow {
    id: String,
    name: LabelRun,
    /// The address in full — what `on_copy_address` reports.
    address: LabelRun,
    /// The same address as [`truncate_address`] renders it, shaped separately
    /// because the row shows the short form and copies the long one.
    short: LabelRun,
    initial: LabelRun,
    /// The row's box inside the panel, in the card's own space.
    rect: Rect,
    /// The copy affordance's box, in the same space.
    copy: Rect,
}

/// Which affordance a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// The account trigger, or the panel header that closes it again.
    Trigger,
    /// The search icon.
    Search,
    /// The bell.
    Bell,
    /// The balance privacy toggle.
    Privacy,
    /// One account row, by index.
    Account(usize),
    /// One account row's copy affordance, by index.
    Copy(usize),
    /// One recent-search term, by index.
    Recent(usize),
    /// One of the four primary actions, by index.
    Action(usize),
}

/// The retained widget for a [`WalletCardView`].
pub struct WalletCardWidget {
    rows: Vec<AccountRow>,
    config: WalletConfig,
    /// The account the card is showing, as this widget last resolved it.
    selected: usize,
    /// The balance's shaped cells, one per grapheme, so the swap cascades.
    balance_cells: Vec<LabelRun>,
    /// The mask's cells, the same way.
    mask_cells: Vec<LabelRun>,
    balance_label: LabelRun,
    delta: LabelRun,
    recent: Vec<LabelRun>,
    search_empty: LabelRun,
    action_labels: [LabelRun; 4],
    trigger_name: LabelRun,
    /// Which panel is open.
    panel: WalletPanel,
    /// The morph driver both panels share — only one is ever open.
    presence: Presence,
    /// How present that panel is, as of the last paint.
    shown: f64,
    /// The frame the current panel run started on, latched at its first paint —
    /// what `delayChildren` and the row stagger are timed against.
    panel_start: Option<FrameTime>,
    /// The balance's own privacy swap: `0.0` shown, `1.0` masked.
    privacy: Lane,
    /// The frame the privacy cascade started on.
    privacy_start: Option<FrameTime>,
    /// Whether the balance is masked.
    hidden: bool,
    /// The frame the copy affordance last latched its check on.
    copied: Option<(usize, FrameTime)>,
    /// Each action's press shrink.
    press: [Lane; 4],
    /// The boxes the last layout resolved, in the card's own space.
    trigger: Rect,
    search_button: Rect,
    bell: Rect,
    privacy_button: Rect,
    panel_box: Rect,
    actions: [Rect; 4],
    recent_rects: Vec<Rect>,
    /// The affordance a `Down` armed.
    armed: Option<Target>,
    /// Whether the account switcher fans its rows in real depth.
    #[cfg(feature = "gpu-effects")]
    gpu_fan: bool,
    /// The fan's own claim on the GPU, held for the widget's life and released
    /// from `View::teardown`.
    #[cfg(feature = "gpu-effects")]
    fan: Card3d,
    on_account_change: ErasedArgCallback<String>,
    on_action: ErasedArgCallback<WalletAction>,
    on_search_submit: ErasedArgCallback<String>,
    on_copy_address: ErasedArgCallback<String>,
    on_notifications: ErasedCallback,
}

impl WalletCardWidget {
    /// Which panel is open.
    pub fn open_panel(&self) -> WalletPanel {
        self.panel
    }

    /// The index of the account the card is showing.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Whether the balance is masked.
    pub fn is_balance_hidden(&self) -> bool {
        self.hidden
    }

    /// The trigger's box in the card's own space — the morph's origin.
    pub fn trigger_rect(&self) -> Rect {
        self.trigger
    }

    /// The open panel's box in the same space — the morph's destination.
    pub fn panel_rect(&self) -> Rect {
        self.panel_box
    }

    /// Open `panel`, or close whatever is open when it is
    /// [`WalletPanel::None`]. Reports whether anything changed.
    fn set_panel(&mut self, panel: WalletPanel) -> bool {
        if self.panel == panel {
            return false;
        }
        // Both panels morph out of the same box, so a switch closes first: two
        // shapes cannot occupy the one travelling rect.
        self.panel = panel;
        self.presence.set_open(panel != WalletPanel::None);
        self.panel_start = None;
        true
    }

    /// The account the card is showing.
    fn account(&self) -> Option<&AccountRow> {
        self.rows.get(self.selected)
    }

    /// The stagger the open panel's rows reveal on.
    fn list_stagger(&self, reduce: bool) -> Stagger {
        let base = Stagger::sprung(WALLET_ITEM_STAGGER, SPRING_PANEL).direction(
            if self.panel == WalletPanel::None {
                StaggerDirection::Exit
            } else {
                StaggerDirection::Enter
            },
        );
        if reduce { base.collapsed() } else { base }
    }

    /// How many rows the open panel lists.
    fn row_count(&self) -> usize {
        match self.panel {
            WalletPanel::Accounts => self.rows.len(),
            WalletPanel::Search => self.recent.len(),
            WalletPanel::None => 0,
        }
    }

    /// How far each open-panel row has been revealed, in row order.
    ///
    /// `delayChildren: 0.12` is measured from the frame the panel opened, so
    /// the run is timed against that latch rather than against the clock. One
    /// source of truth for it, because the flat rows and the 3D plates have to
    /// arrive together.
    fn row_reveals(&self, now: FrameTime, reduce: bool) -> Vec<f64> {
        let stagger = self.list_stagger(reduce);
        let elapsed = now
            .saturating_sub(self.panel_start.unwrap_or(now))
            .saturating_sub(WALLET_LIST_DELAY);
        let count = self.row_count();
        (0..count)
            .map(|index| {
                if reduce {
                    1.0
                } else {
                    stagger.revealed(elapsed, index, count) * self.shown
                }
            })
            .collect()
    }

    /// The open panel's own height, in logical px.
    fn panel_height(&self) -> f64 {
        let rows = self.row_count();
        if rows == 0 {
            return WALLET_HEAD_HEIGHT + WALLET_ROW_HEIGHT;
        }
        let list = (rows as f64 * WALLET_ROW_HEIGHT + WALLET_LIST_PADDING * 2.0)
            .min(WALLET_LIST_MAX_HEIGHT);
        WALLET_HEAD_HEIGHT + list
    }

    /// The affordance `position` lands on, if any.
    fn target_at(&self, position: Point) -> Option<Target> {
        if self.panel != WalletPanel::None && self.panel_box.contains(position) {
            if position.y <= self.panel_box.y0 + WALLET_HEAD_HEIGHT {
                return Some(Target::Trigger);
            }
            for (index, rect) in self.recent_rects.iter().enumerate() {
                if rect.width() <= 0.0 || !rect.contains(position) {
                    continue;
                }
                return Some(match self.panel {
                    WalletPanel::Search => Target::Recent(index),
                    _ => {
                        if self.rows[index].copy.contains(position) {
                            Target::Copy(index)
                        } else {
                            Target::Account(index)
                        }
                    }
                });
            }
            return Some(Target::Trigger);
        }
        if self.trigger.contains(position) {
            return Some(Target::Trigger);
        }
        if self.search_button.contains(position) {
            return Some(Target::Search);
        }
        if self.bell.contains(position) {
            return Some(Target::Bell);
        }
        if self.privacy_button.contains(position) {
            return Some(Target::Privacy);
        }
        self.actions
            .iter()
            .position(|rect| rect.contains(position))
            .map(Target::Action)
    }

    /// Act on a released press.
    fn fire(&mut self, ctx: &mut EventCtx, target: Target) {
        match target {
            Target::Trigger => {
                let next = if self.panel == WalletPanel::Accounts {
                    WalletPanel::None
                } else {
                    WalletPanel::Accounts
                };
                self.set_panel(next);
            }
            Target::Search => {
                let next = if self.panel == WalletPanel::Search {
                    WalletPanel::None
                } else {
                    WalletPanel::Search
                };
                self.set_panel(next);
            }
            Target::Bell => (self.on_notifications)(ctx),
            Target::Privacy => {
                self.hidden = !self.hidden;
                self.privacy.retarget(if self.hidden { 1.0 } else { 0.0 });
                self.privacy_start = None;
            }
            Target::Account(index) => {
                if let Some(row) = self.rows.get(index) {
                    let id = row.id.clone();
                    if self.config.account_id.is_none() {
                        self.selected = index;
                    }
                    self.set_panel(WalletPanel::None);
                    (self.on_account_change)(ctx, id);
                }
            }
            Target::Copy(index) => {
                if let Some(row) = self.rows.get(index) {
                    let address = row.address.content().to_string();
                    // The check is latched at the next paint, which is the only
                    // pass with a clock.
                    self.copied = Some((index, FrameTime::ZERO));
                    (self.on_copy_address)(ctx, address);
                }
            }
            Target::Recent(index) => {
                if let Some(term) = self.recent.get(index) {
                    let term = term.content().to_string();
                    self.set_panel(WalletPanel::None);
                    (self.on_search_submit)(ctx, term);
                }
            }
            Target::Action(index) => {
                (self.on_action)(ctx, WalletAction::ALL[index.min(3)]);
            }
        }
        ctx.request_redraw();
    }
}

/// Build the retained rows for a declared account list.
fn account_rows(accounts: &[WalletAccount]) -> Vec<AccountRow> {
    accounts
        .iter()
        .map(|account| AccountRow {
            id: account.id.clone(),
            name: LabelRun::new(account.name.clone()),
            address: LabelRun::new(account.address.clone()),
            short: LabelRun::new(truncate_address(&account.address)),
            initial: LabelRun::new(account.initial()),
            rect: Rect::ZERO,
            copy: Rect::ZERO,
        })
        .collect()
}

/// One shaped run per grapheme of `text`.
fn cells(text: &str) -> Vec<LabelRun> {
    text.chars()
        .map(|ch| LabelRun::new(ch.to_string()))
        .collect()
}

/// The index of the account `id` names, or zero.
fn index_of(accounts: &[WalletAccount], id: Option<&String>) -> usize {
    let Some(id) = id else { return 0 };
    accounts
        .iter()
        .position(|account| &account.id == id)
        .unwrap_or(0)
}

impl<State: 'static> View<State> for WalletCardView<State> {
    type Element = WalletCardWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> WalletCardWidget {
        let selected = index_of(&self.accounts, self.config.account_id.as_ref());
        let rows = account_rows(&self.accounts);
        let name = rows
            .get(selected)
            .map_or_else(String::new, |row| row.name.content().to_string());
        WalletCardWidget {
            balance_cells: cells(&format_balance(
                self.config.balance,
                &self.config.balance_prefix,
            )),
            mask_cells: cells(WALLET_MASK),
            balance_label: LabelRun::new("Balance"),
            delta: LabelRun::new(self.config.change.map_or_else(String::new, format_delta)),
            recent: self
                .config
                .search_recent
                .iter()
                .map(|term| LabelRun::new(term.clone()))
                .collect(),
            search_empty: LabelRun::new(self.config.search_empty_label.clone()),
            action_labels: [
                LabelRun::new(WalletAction::Send.label()),
                LabelRun::new(WalletAction::Deposit.label()),
                LabelRun::new(WalletAction::Swap.label()),
                LabelRun::new(WalletAction::Buy.label()),
            ],
            trigger_name: LabelRun::new(name),
            rows,
            selected,
            config: self.config.clone(),
            panel: WalletPanel::None,
            presence: Presence::symmetric(WALLET_MORPH),
            shown: 0.0,
            panel_start: None,
            privacy: Lane::at_rest(
                Ramp::eased(Duration::from_millis(200), EASE_OUT),
                if self.config.balance_hidden { 1.0 } else { 0.0 },
            ),
            privacy_start: None,
            hidden: self.config.balance_hidden,
            copied: None,
            press: [
                Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
                Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
                Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
                Lane::at_rest(Ramp::spring(SPRING_PRESS), 0.0),
            ],
            trigger: Rect::ZERO,
            search_button: Rect::ZERO,
            bell: Rect::ZERO,
            privacy_button: Rect::ZERO,
            panel_box: Rect::ZERO,
            actions: [Rect::ZERO; 4],
            recent_rects: Vec::new(),
            armed: None,
            #[cfg(feature = "gpu-effects")]
            gpu_fan: self.gpu_fan,
            #[cfg(feature = "gpu-effects")]
            fan: Card3d::new(FX_LABEL),
            on_account_change: erase_callback_arg(&self.on_account_change),
            on_action: erase_callback_arg(&self.on_action),
            on_search_submit: erase_callback_arg(&self.on_search_submit),
            on_copy_address: erase_callback_arg(&self.on_copy_address),
            on_notifications: erase_callback(&self.on_notifications),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut WalletCardWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.accounts != self.accounts {
            element.rows = account_rows(&self.accounts);
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.config != self.config {
            if element.config.balance != self.config.balance
                || element.config.balance_prefix != self.config.balance_prefix
            {
                element.balance_cells = cells(&format_balance(
                    self.config.balance,
                    &self.config.balance_prefix,
                ));
                // A moved balance re-cascades, which is what `ActionSwapText`
                // does with a changed value.
                element.privacy_start = None;
            }
            if element.config.balance_hidden != self.config.balance_hidden {
                element.hidden = self.config.balance_hidden;
                element
                    .privacy
                    .retarget(if element.hidden { 1.0 } else { 0.0 });
                element.privacy_start = None;
            }
            element
                .delta
                .set_content(self.config.change.map_or_else(String::new, format_delta));
            element.recent = self
                .config
                .search_recent
                .iter()
                .map(|term| LabelRun::new(term.clone()))
                .collect();
            element
                .search_empty
                .set_content(self.config.search_empty_label.clone());
            element.config = self.config.clone();
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if let Some(id) = self.config.account_id.as_ref() {
            element.selected = index_of(&self.accounts, Some(id));
        }
        let name = element
            .rows
            .get(element.selected)
            .map_or_else(String::new, |row| row.name.content().to_string());
        element.trigger_name.set_content(name);
        #[cfg(feature = "gpu-effects")]
        if element.gpu_fan != self.gpu_fan {
            element.gpu_fan = self.gpu_fan;
            if !self.gpu_fan {
                // Withdrawn mid-life: the composite goes on the next drained
                // frame, and the rows fall back to their 2D plate.
                element.fan.clear();
            }
            flags |= ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapters is cheap.
        element.on_account_change = erase_callback_arg(&self.on_account_change);
        element.on_action = erase_callback_arg(&self.on_action);
        element.on_search_submit = erase_callback_arg(&self.on_search_submit);
        element.on_copy_address = erase_callback_arg(&self.on_copy_address);
        element.on_notifications = erase_callback(&self.on_notifications);
        flags
    }

    #[cfg_attr(not(feature = "gpu-effects"), allow(unused_variables))]
    fn teardown(&self, element: &mut WalletCardWidget, _ctx: &mut BuildCtx<'_>) {
        #[cfg(feature = "gpu-effects")]
        element.fan.release();
    }
}

/// One label style at `size`, in the theme's `label_large` family.
fn wallet_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    themed_style(
        crate::text::label_style(size),
        ThemeTextType::LabelLarge,
        theme,
    )
}

impl Widget for WalletCardWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        // Every style is resolved before the first `layout` call: the shaper
        // takes `ctx` mutably, and the theme read borrows it.
        let body = wallet_style(theme, style::TEXT_SM);
        let small = wallet_style(theme, style::TEXT_XS);
        let big = wallet_style(theme, WALLET_BALANCE_SIZE);

        self.balance_label.layout(ctx, &small);
        self.delta.layout(ctx, &small);
        self.trigger_name.layout(ctx, &body);
        self.search_empty.layout(ctx, &body);
        for cell in self
            .balance_cells
            .iter_mut()
            .chain(self.mask_cells.iter_mut())
        {
            cell.layout(ctx, &big);
        }
        for label in &mut self.action_labels {
            label.layout(ctx, &small);
        }
        for term in &mut self.recent {
            term.layout(ctx, &body);
        }
        for row in &mut self.rows {
            row.name.layout(ctx, &body);
            row.short.layout(ctx, &small);
            row.initial.layout(ctx, &small);
        }

        let width = bc.constrain(Size::new(WALLET_WIDTH, 0.0)).width;
        let inner = (width - WALLET_PADDING * 2.0).max(0.0);

        // The header row: the trigger on the left, two icon buttons right.
        let head_y = WALLET_PADDING;
        let icons = WALLET_ICON_BUTTON * 2.0 + style::SPACING_UNIT;
        self.trigger = Rect::from_origin_size(
            Point::new(WALLET_PADDING - WALLET_HEAD_PADDING_X, head_y),
            Size::new(
                (inner - icons + WALLET_HEAD_PADDING_X).max(0.0),
                WALLET_HEAD_HEIGHT,
            ),
        );
        let icons_x = WALLET_PADDING + inner - icons;
        self.search_button = Rect::from_origin_size(
            Point::new(
                icons_x,
                head_y + (WALLET_HEAD_HEIGHT - WALLET_ICON_BUTTON) / 2.0,
            ),
            Size::new(WALLET_ICON_BUTTON, WALLET_ICON_BUTTON),
        );
        self.bell = Rect::from_origin_size(
            Point::new(
                icons_x + WALLET_ICON_BUTTON + style::SPACING_UNIT,
                self.search_button.y0,
            ),
            Size::new(WALLET_ICON_BUTTON, WALLET_ICON_BUTTON),
        );

        // Either panel spans the header row's full width (upstream's
        // `-right-2 -left-2`) and grows downward from its top.
        self.panel_box = Rect::from_origin_size(
            Point::new(WALLET_PADDING - WALLET_HEAD_PADDING_X, head_y),
            Size::new(inner + WALLET_HEAD_PADDING_X * 2.0, self.panel_height()),
        );

        // The balance block.
        let balance_y = head_y + WALLET_HEAD_HEIGHT + WALLET_BLOCK_GAP;
        let label = self.balance_label.size();
        self.privacy_button = Rect::from_origin_size(
            Point::new(
                WALLET_PADDING + (inner + label.width) / 2.0 + style::GAP_SM,
                balance_y,
            ),
            Size::new(style::ICON_SIZE, label.height.max(style::ICON_SIZE)),
        );
        let cell_height = self
            .balance_cells
            .first()
            .map_or(WALLET_BALANCE_SIZE, |cell| cell.size().height);
        let actions_y = balance_y
            + label.height
            + style::GAP_SM
            + cell_height
            + WALLET_DELTA_HEIGHT
            + WALLET_BLOCK_GAP;

        // The four actions, evenly spread across the card.
        let slot = inner / 4.0;
        for index in 0..4 {
            self.actions[index] = Rect::from_origin_size(
                Point::new(
                    WALLET_PADDING + slot * index as f64 + (slot - WALLET_ACTION_SIZE) / 2.0,
                    actions_y,
                ),
                Size::new(WALLET_ACTION_SIZE, WALLET_ACTION_SIZE),
            );
        }
        let label_height = self.action_labels[0].size().height;
        let height =
            actions_y + WALLET_ACTION_SIZE + WALLET_ACTION_GAP + label_height + WALLET_PADDING;

        // The open panel's rows, in the card's own space.
        let rows = self.row_count();
        self.recent_rects = (0..rows)
            .map(|index| {
                Rect::from_origin_size(
                    Point::new(
                        self.panel_box.x0 + WALLET_LIST_PADDING,
                        self.panel_box.y0
                            + WALLET_HEAD_HEIGHT
                            + WALLET_LIST_PADDING
                            + index as f64 * WALLET_ROW_HEIGHT,
                    ),
                    Size::new(
                        (self.panel_box.width() - WALLET_LIST_PADDING * 2.0).max(0.0),
                        WALLET_ROW_HEIGHT,
                    ),
                )
            })
            .collect();
        if self.panel == WalletPanel::Accounts {
            for (index, rect) in self.recent_rects.iter().enumerate() {
                let row = &mut self.rows[index];
                row.rect = *rect;
                row.copy = Rect::from_origin_size(
                    Point::new(
                        rect.x1 - WALLET_ICON_BUTTON,
                        rect.y0 + (rect.height() - WALLET_ICON_BUTTON) / 2.0,
                    ),
                    Size::new(WALLET_ICON_BUTTON, WALLET_ICON_BUTTON),
                );
            }
        } else {
            for row in &mut self.rows {
                row.rect = Rect::ZERO;
                row.copy = Rect::ZERO;
            }
        }

        bc.constrain(Size::new(width, height))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        // The delta pill's two hues are beUI's own authored `--success` /
        // `--danger`, not Tailwind's emerald and red — the same substitution
        // `crate::components::animated_badge` records for its status pills.
        let tokens = crate::BeuiTokens::resolve(theme);
        let (accent, muted, success, danger) = match theme {
            Some(t) => (
                t.scheme().primary,
                t.scheme().surface_container_highest,
                tokens.success,
                t.scheme().error,
            ),
            None => (
                crate::BEUI_LIGHT.primary,
                crate::BEUI_LIGHT.muted,
                tokens.success,
                crate::BEUI_LIGHT.destructive,
            ),
        };
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        if reduce {
            self.presence = self.presence.collapsed();
            self.privacy.snap();
            for lane in &mut self.press {
                lane.snap();
            }
        }
        self.panel_start.get_or_insert(now);
        self.shown = self.presence.advance(now);
        if self.presence.is_animating() {
            ctx.request_layout();
        }
        if !reduce {
            let mut animating = self.privacy.advance(now);
            for lane in &mut self.press {
                animating |= lane.advance(now);
            }
            if animating {
                ctx.request_frame();
            }
        }
        if self.config.has_notifications && !reduce {
            // The unread halo is a decorative loop, not a transition.
            ctx.request_frame_class(TickClass::CosmeticLoop);
        }

        // The card itself.
        let radius = style::resolve_radius(WALLET_RADIUS, size.width, size.height);
        scene.fill_rounded_rect(origin, size, radius, chrome.surface);
        paint_panel_hairline(scene, origin, size, radius, chrome.border);

        // The header row: the trigger under whichever panel is growing from it.
        self.paint_trigger(scene, origin, chrome, accent);
        self.paint_icon_buttons(scene, origin, chrome, accent, now, reduce);
        self.paint_balance(scene, origin, size, chrome, success, danger, now, reduce);
        self.paint_actions(scene, origin, chrome, muted);
        if self.shown > 0.0 {
            self.paint_panel(ctx, scene, origin, chrome, accent, muted, now, reduce);
        } else {
            // Nothing to fan while the panel is shut, and a pass left holding
            // a scene would keep compositing one.
            self.clear_fan();
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                if key.key == Key::Named(NamedKey::Escape) && self.panel != WalletPanel::None {
                    self.set_panel(WalletPanel::None);
                    ctx.request_redraw();
                    return EventResult::Handled;
                }
                if is_activation_key(key) && self.panel == WalletPanel::None {
                    self.fire(ctx, Target::Trigger);
                    return EventResult::Handled;
                }
                EventResult::Ignored
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let name = self.trigger_name.content().to_string();
        let balance = if self.hidden {
            "Balance hidden".to_string()
        } else {
            format_balance(self.config.balance, &self.config.balance_prefix)
        };
        let open = self.panel != WalletPanel::None;
        ctx.push_container(
            Role::Group,
            |node| {
                node.set_label("Wallet");
            },
            |ctx| {
                ctx.push_node(Role::Button, |node| {
                    node.set_label(name.as_str());
                    node.set_expanded(open && self.panel == WalletPanel::Accounts);
                    node.add_action(Action::Click);
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label("Search");
                    node.set_expanded(open && self.panel == WalletPanel::Search);
                    node.add_action(Action::Click);
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label("Notifications");
                    node.add_action(Action::Click);
                });
                ctx.push_node(Role::Label, |node| {
                    node.set_label(balance.as_str());
                });
                ctx.push_node(Role::Button, |node| {
                    node.set_label(if self.hidden {
                        "Show balance"
                    } else {
                        "Hide balance"
                    });
                    node.add_action(Action::Click);
                });
                // The panel's own rows are only announced while input can reach
                // them — the input-parity carve-out `docs/CODE_STANDARDS.md`'s
                // Semantics Conventions name.
                match self.panel {
                    WalletPanel::Accounts => {
                        for (index, row) in self.rows.iter().enumerate() {
                            let label = format!(
                                "{}, {}",
                                row.name.content(),
                                truncate_address(row.address.content())
                            );
                            let selected = index == self.selected;
                            ctx.push_node(Role::ListBoxOption, |node| {
                                node.set_label(label.as_str());
                                node.set_selected(selected);
                                node.add_action(Action::Click);
                            });
                        }
                    }
                    WalletPanel::Search => {
                        for term in &self.recent {
                            let label = term.content().to_string();
                            ctx.push_node(Role::ListBoxOption, |node| {
                                node.set_label(label.as_str());
                                node.add_action(Action::Click);
                            });
                        }
                    }
                    WalletPanel::None => {}
                }
                for (index, action) in WalletAction::ALL.into_iter().enumerate() {
                    let label = self.action_labels[index].content().to_string();
                    let _ = action;
                    ctx.push_node(Role::Button, |node| {
                        node.set_label(label.as_str());
                        node.add_action(Action::Click);
                    });
                }
            },
        );
    }
}

impl WalletCardWidget {
    /// The account trigger: an avatar disc, the name and a chevron.
    fn paint_trigger(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        accent: Color,
    ) {
        // While the panel is fully open the trigger has become the panel; below
        // that it fades out under the travelling box.
        let alpha = (1.0 - self.shown).clamp(0.0, 1.0) as f32;
        if alpha <= 0.0 {
            return;
        }
        let at = origin + self.trigger.origin().to_vec2();
        scene.push_layer(at, self.trigger.size(), alpha);
        paint_avatar(
            scene,
            Point::new(at.x + WALLET_HEAD_PADDING_X, at.y + WALLET_HEAD_PADDING_Y),
            self.account().map(|row| &row.initial),
            accent,
            chrome.surface,
        );
        let name = self.trigger_name.size();
        let text_x = at.x + WALLET_HEAD_PADDING_X + WALLET_AVATAR_SIZE + WALLET_HEAD_GAP;
        self.trigger_name.paint(
            Point::new(text_x, at.y + (self.trigger.height() - name.height) / 2.0),
            chrome.ink,
            scene,
        );
        draw_chevron(
            scene,
            Point::new(
                text_x + name.width + WALLET_HEAD_GAP + style::ICON_SIZE / 2.0,
                at.y + self.trigger.height() / 2.0,
            ),
            style::ICON_SIZE,
            0.0,
            chrome.dim_ink,
        );
        scene.pop_layer();
    }

    /// The search icon and the bell, with its unread pulse.
    fn paint_icon_buttons(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        accent: Color,
        now: FrameTime,
        reduce: bool,
    ) {
        let alpha = (1.0 - self.shown).clamp(0.0, 1.0) as f32;
        if alpha > 0.0 {
            let at = origin + self.search_button.origin().to_vec2();
            scene.push_layer(at, self.search_button.size(), alpha);
            draw_search(
                scene,
                Point::new(
                    at.x + WALLET_ICON_BUTTON / 2.0,
                    at.y + WALLET_ICON_BUTTON / 2.0,
                ),
                style::ICON_SIZE,
                chrome.dim_ink,
            );
            scene.pop_layer();
        }

        let bell_at = origin + self.bell.origin().to_vec2();
        let bell_centre = Point::new(
            bell_at.x + WALLET_ICON_BUTTON / 2.0,
            bell_at.y + WALLET_ICON_BUTTON / 2.0,
        );
        draw_bell(scene, bell_centre, style::ICON_SIZE, chrome.dim_ink);
        if self.config.has_notifications {
            let dot = Point::new(bell_centre.x + 6.0, bell_centre.y - 6.0);
            let radius = 4.0;
            if !reduce {
                let (scale, halo) = wallet_pulse(now.saturating_sub(FrameTime::ZERO));
                let reach = radius * scale;
                scene.fill_rounded_rect(
                    Point::new(dot.x - reach, dot.y - reach),
                    Size::new(reach * 2.0, reach * 2.0),
                    reach,
                    style::with_alpha(accent, (halo * 0.6) as f32),
                );
            }
            scene.fill_rounded_rect(
                Point::new(dot.x - radius, dot.y - radius),
                Size::new(radius * 2.0, radius * 2.0),
                radius,
                accent,
            );
        }
    }

    /// The balance block: its label, its privacy toggle, the cascading number
    /// and the delta pill.
    #[allow(clippy::too_many_arguments)]
    fn paint_balance(
        &mut self,
        scene: &mut dyn PaintScene,
        origin: Point,
        size: Size,
        chrome: PanelChrome,
        success: Color,
        danger: Color,
        now: FrameTime,
        reduce: bool,
    ) {
        let centre_x = origin.x + size.width / 2.0;
        let label = self.balance_label.size();
        let label_y = origin.y + self.privacy_button.y0;
        self.balance_label.paint(
            Point::new(
                centre_x - (label.width + style::GAP_SM + style::ICON_SIZE) / 2.0,
                label_y,
            ),
            chrome.dim_ink,
            scene,
        );
        draw_eye(
            scene,
            Point::new(
                origin.x + self.privacy_button.center().x,
                origin.y + self.privacy_button.center().y,
            ),
            style::ICON_SIZE,
            self.hidden,
            chrome.dim_ink,
        );

        // The number and the mask cascade past each other, one letter at a
        // time — the `ActionSwapText` swap upstream drives it with.
        let started = *self.privacy_start.get_or_insert(now);
        let elapsed = if reduce {
            Duration::from_secs(60)
        } else {
            now.saturating_sub(started)
        };
        let swap = self.privacy.value().clamp(0.0, 1.0);
        let cascade = Stagger::sprung(WALLET_ITEM_STAGGER, SPRING_PANEL);
        let (shown, going) = if self.hidden {
            (&self.mask_cells, &self.balance_cells)
        } else {
            (&self.balance_cells, &self.mask_cells)
        };
        let y = label_y + label.height + style::GAP_SM;
        paint_cells(
            scene, centre_x, y, shown, chrome.ink, &cascade, elapsed, 1.0,
        );
        if swap > 0.0 && swap < 1.0 {
            paint_cells(
                scene,
                centre_x,
                y,
                going,
                chrome.ink,
                &cascade,
                elapsed,
                1.0 - swap,
            );
        }

        // The delta pill, hidden with the balance.
        let height = shown.first().map_or(0.0, |cell| cell.size().height);
        if self.config.change.is_some() && !self.hidden {
            let delta = self.delta.size();
            let pill = Size::new(
                delta.width + style::PADDING_X_SM * 2.0,
                WALLET_DELTA_HEIGHT - style::SPACING_UNIT,
            );
            let tone = WalletDeltaTone::of(self.config.change.unwrap_or_default());
            let ink = match tone {
                WalletDeltaTone::Up => success,
                WalletDeltaTone::Down => danger,
            };
            let at = Point::new(
                centre_x - pill.width / 2.0,
                y + height + (WALLET_DELTA_HEIGHT - pill.height) / 2.0,
            );
            scene.fill_rounded_rect(
                at,
                pill,
                style::resolve_radius(style::RADIUS_CONTROL, pill.width, pill.height),
                style::with_alpha(ink, WALLET_DELTA_WASH_ALPHA),
            );
            self.delta.paint(
                Point::new(
                    at.x + style::PADDING_X_SM,
                    at.y + (pill.height - delta.height) / 2.0,
                ),
                ink,
                scene,
            );
        }
    }

    /// The four primary actions, icon over label.
    fn paint_actions(
        &self,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        muted: Color,
    ) {
        for index in 0..4 {
            let rect = self.actions[index];
            let scale = press_scale(WALLET_PRESS_SCALE, self.press[index].value());
            let disc = Size::new(rect.width() * scale, rect.height() * scale);
            let at = origin
                + Vec2::new(
                    rect.x0 + (rect.width() - disc.width) / 2.0,
                    rect.y0 + (rect.height() - disc.height) / 2.0,
                );
            scene.fill_rounded_rect(at, disc, disc.width / 2.0, muted);
            draw_action_mark(
                scene,
                Point::new(at.x + disc.width / 2.0, at.y + disc.height / 2.0),
                style::ICON_SIZE_LG * scale,
                WalletAction::ALL[index],
                chrome.ink,
            );
            let label = self.action_labels[index].size();
            self.action_labels[index].paint(
                origin
                    + Vec2::new(
                        rect.center().x - label.width / 2.0,
                        rect.y1 + WALLET_ACTION_GAP,
                    ),
                chrome.dim_ink,
                scene,
            );
        }
    }

    /// The open panel: the travelling box, its header and its staggered rows.
    #[allow(clippy::too_many_arguments)]
    fn paint_panel(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        origin: Point,
        chrome: PanelChrome,
        accent: Color,
        muted: Color,
        now: FrameTime,
        reduce: bool,
    ) {
        let (rect, radius) = morph_rect(
            self.trigger,
            WALLET_ICON_RADIUS,
            self.panel_box,
            WALLET_PANEL_RADIUS,
            self.shown,
        );
        let at = origin + rect.origin().to_vec2();
        scene.fill_rounded_rect(at, rect.size(), radius, chrome.surface);
        paint_panel_hairline(scene, at, rect.size(), radius, chrome.border);
        scene.push_clip_rounded(at, rect.size(), radius);

        // The header, which is the trigger that grew into the panel.
        paint_avatar(
            scene,
            Point::new(at.x + WALLET_HEAD_PADDING_X, at.y + WALLET_HEAD_PADDING_Y),
            self.account().map(|row| &row.initial),
            accent,
            chrome.surface,
        );
        let name = self.trigger_name.size();
        self.trigger_name.paint(
            Point::new(
                at.x + WALLET_HEAD_PADDING_X + WALLET_AVATAR_SIZE + WALLET_HEAD_GAP,
                at.y + (WALLET_HEAD_HEIGHT - name.height) / 2.0,
            ),
            chrome.ink,
            scene,
        );

        // The rows, staggered in behind `delayChildren`.
        let reveals = self.row_reveals(now, reduce);
        // The account rows as a genuinely fanned stack, when the 3D path is
        // live: the plates then come from the composite rather than from the
        // 2D fill below.
        let fanned = self.paint_fan(ctx, scene, origin, &reveals, muted);
        let count = self.row_count();
        for index in 0..count {
            let Some(row_rect) = self.recent_rects.get(index) else {
                continue;
            };
            let reveal = reveals.get(index).copied().unwrap_or(0.0);
            if reveal <= 0.0 {
                continue;
            }
            let slide = WALLET_ITEM_TRAVEL * (1.0 - reveal);
            let row_at = origin + row_rect.origin().to_vec2() + Vec2::new(0.0, slide);
            scene.push_layer(row_at, row_rect.size(), reveal.clamp(0.0, 1.0) as f32);
            match self.panel {
                WalletPanel::Accounts => {
                    // The fan draws every row's plate, the selected one lifted;
                    // the flat path picks the selection out with this one.
                    if index == self.selected && !fanned {
                        scene.fill_rounded_rect(row_at, row_rect.size(), style::RADIUS_XL, muted);
                    }
                    let row = &self.rows[index];
                    paint_avatar(
                        scene,
                        Point::new(
                            row_at.x + WALLET_HEAD_PADDING_X,
                            row_at.y + (row_rect.height() - WALLET_AVATAR_SIZE) / 2.0,
                        ),
                        Some(&row.initial),
                        accent,
                        chrome.surface,
                    );
                    let text_x =
                        row_at.x + WALLET_HEAD_PADDING_X + WALLET_AVATAR_SIZE + WALLET_HEAD_GAP;
                    row.name.paint(
                        Point::new(text_x, row_at.y + style::GAP_SM),
                        chrome.ink,
                        scene,
                    );
                    // The address is truncated for display only — the full one
                    // is what `on_copy_address` reports.
                    row.short.paint(
                        Point::new(
                            text_x,
                            row_at.y + row_rect.height() - style::GAP_MD - row.short.size().height,
                        ),
                        chrome.dim_ink,
                        scene,
                    );
                    let copied = self.copied.is_some_and(|(slot, _)| slot == index);
                    draw_copy(
                        scene,
                        Point::new(
                            row_at.x + row.copy.x0 - row_rect.x0 + WALLET_ICON_BUTTON / 2.0,
                            row_at.y + row_rect.height() / 2.0,
                        ),
                        style::ICON_SIZE,
                        copied,
                        if copied { accent } else { chrome.dim_ink },
                    );
                }
                WalletPanel::Search => {
                    let term = &self.recent[index];
                    let text = term.size();
                    term.paint(
                        Point::new(
                            row_at.x + WALLET_HEAD_PADDING_X,
                            row_at.y + (row_rect.height() - text.height) / 2.0,
                        ),
                        chrome.dim_ink,
                        scene,
                    );
                }
                WalletPanel::None => {}
            }
            scene.pop_layer();
        }
        if count == 0 && self.panel == WalletPanel::Search {
            let text = self.search_empty.size();
            self.search_empty.paint(
                Point::new(
                    at.x + (rect.width() - text.width) / 2.0,
                    at.y + WALLET_HEAD_HEIGHT + (WALLET_ROW_HEIGHT - text.height) / 2.0,
                ),
                chrome.dim_ink,
                scene,
            );
        }
        scene.pop_clip();
    }

    /// The `Widget::event` pointer arm.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        let over = self.target_at(p.position);
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
                if !inside(p.position, size) {
                    // A press outside the card dismisses whatever is open,
                    // which is upstream's `useDismiss`.
                    if self.set_panel(WalletPanel::None) {
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    return EventResult::Ignored;
                }
                let Some(target) = over else {
                    if self.set_panel(WalletPanel::None) {
                        ctx.request_redraw();
                        return EventResult::Handled;
                    }
                    return EventResult::Ignored;
                };
                ctx.capture_pointer();
                self.armed = Some(target);
                if let Target::Action(index) = target {
                    self.press[index].retarget(1.0);
                    ctx.request_redraw();
                }
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if let Target::Action(index) = armed {
                    self.press[index].retarget(0.0);
                    ctx.request_redraw();
                }
                if over == Some(armed) {
                    self.fire(ctx, armed);
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if let Target::Action(index) = armed {
                    self.press[index].retarget(0.0);
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

#[cfg(feature = "gpu-effects")]
impl WalletCardWidget {
    /// Render the open account list as a depth-fanned stack of real
    /// perspective plates, answering whether it took the rows' surfaces over.
    ///
    /// `false` — and therefore the ordinary 2D plate — whenever the caller did
    /// not opt in, the open panel is not the account switcher, the GPU is
    /// unreachable, the panel has no usable box, or no row has arrived yet.
    /// Every one of those is an ordinary frame, not an error.
    fn paint_fan(
        &mut self,
        ctx: &mut PaintCtx,
        scene: &mut dyn PaintScene,
        origin: Point,
        reveals: &[f64],
        muted: Color,
    ) -> bool {
        if !self.gpu_fan || self.panel != WalletPanel::Accounts {
            self.fan.clear();
            return false;
        }
        if !self.fan.acquire() {
            return false;
        }
        let panel = self.panel_box;
        let size = panel.size();
        let Some((dest, extent)) = card3d::target_for(origin + panel.origin().to_vec2(), size)
        else {
            self.fan.clear();
            return false;
        };
        let cards = self.fan_cards(reveals, muted, extent);
        if cards.is_empty() {
            self.fan.clear();
            return false;
        }

        // See `Card3d::submit`'s doc for bind timing and why a `true` answer
        // asks for a frame.
        let first = self.fan.submit(extent, card3d::fan_scene(cards));
        request_frame(ctx, card3d::cadence(first));
        let Some(id) = self.fan.scene_texture_id() else {
            return false;
        };
        scene.draw_scene_texture(id, dest);
        true
    }

    /// The plates the open account list fans, in **row order** — not depth
    /// order. That is not merely tolerated: `card3d::fan_scene` sorts them
    /// nearest-first itself before they reach the renderer (translucent
    /// plates need that order, not just the depth attachment, to occlude
    /// without darkening at a seam), so this caller's own order is free to
    /// carry no depth meaning at all.
    ///
    /// A row that has not begun arriving contributes nothing; the rest carry
    /// the stagger's own slide and its reveal as their alpha.
    fn fan_cards(&self, reveals: &[f64], muted: Color, extent: (u32, u32)) -> Vec<FanCard> {
        let panel = self.panel_box;
        let size = panel.size();
        reveals
            .iter()
            .enumerate()
            .filter_map(|(index, reveal)| {
                let reveal = reveal.clamp(0.0, 1.0);
                if reveal <= 0.0 {
                    return None;
                }
                let rect = self.recent_rects.get(index)?;
                let slide = WALLET_ITEM_TRAVEL * (1.0 - reveal);
                let in_panel = *rect - panel.origin().to_vec2() + Vec2::new(0.0, slide);
                let plate = if index == self.selected {
                    1.0
                } else {
                    WALLET_FAN_PLATE_ALPHA
                };
                Some(FanCard {
                    dest: card3d::in_target(extent, size, in_panel),
                    face: QuadFace::Solid(style::with_alpha(muted, plate * reveal as f32)),
                    slot: index as i32 - self.selected as i32,
                })
            })
            .collect()
    }

    /// Drop whatever the fan is holding — the panel closed, or the mode was
    /// switched off — without giving up the claim.
    fn clear_fan(&self) {
        self.fan.clear();
    }
}

#[cfg(not(feature = "gpu-effects"))]
impl WalletCardWidget {
    /// Without the `gpu-effects` feature the account list has no 3D path at
    /// all: every row keeps its ordinary 2D plate.
    fn paint_fan(
        &mut self,
        _ctx: &mut PaintCtx,
        _scene: &mut dyn PaintScene,
        _origin: Point,
        _reveals: &[f64],
        _muted: Color,
    ) -> bool {
        false
    }

    /// Nothing to drop without the `gpu-effects` feature.
    fn clear_fan(&self) {}
}

/// Paint the cells of one text run, centred on `centre_x`, each cell staged by
/// `cascade` and faded by `alpha`.
#[allow(clippy::too_many_arguments)]
fn paint_cells(
    scene: &mut dyn PaintScene,
    centre_x: f64,
    y: f64,
    cells: &[LabelRun],
    ink: Color,
    cascade: &Stagger,
    elapsed: Duration,
    alpha: f64,
) {
    let width: f64 = cells.iter().map(|cell| cell.size().width).sum();
    let mut x = centre_x - width / 2.0;
    for (index, cell) in cells.iter().enumerate() {
        let reveal = cascade.revealed(elapsed, index, cells.len()) * alpha;
        if reveal > 0.0 {
            cell.paint(
                Point::new(x, y),
                style::with_alpha(ink, reveal.clamp(0.0, 1.0) as f32),
                scene,
            );
        }
        x += cell.size().width;
    }
}

/// Paint an account's disc: a tinted circle carrying its initial.
fn paint_avatar(
    scene: &mut dyn PaintScene,
    at: Point,
    initial: Option<&LabelRun>,
    accent: Color,
    on_accent: Color,
) {
    scene.fill_rounded_rect(
        at,
        Size::new(WALLET_AVATAR_SIZE, WALLET_AVATAR_SIZE),
        WALLET_AVATAR_SIZE / 2.0,
        style::with_alpha(accent, 0.20),
    );
    let _ = on_accent;
    if let Some(initial) = initial {
        let text = initial.size();
        initial.paint(
            Point::new(
                at.x + (WALLET_AVATAR_SIZE - text.width) / 2.0,
                at.y + (WALLET_AVATAR_SIZE - text.height) / 2.0,
            ),
            accent,
            scene,
        );
    }
}

/// lucide's own viewBox extent.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// lucide's default `strokeWidth`, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

/// Paint lucide's `chevron-down`, rotated by `turn` half-turns.
fn draw_chevron(scene: &mut dyn PaintScene, centre: Point, extent: f64, turn: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let dir = if turn >= 0.5 { -1.0 } else { 1.0 };
    let mut path = BezPath::new();
    path.move_to(Point::new(-6.0 * scale, -3.0 * scale * dir));
    path.line_to(Point::new(0.0, 3.0 * scale * dir));
    path.line_to(Point::new(6.0 * scale, -3.0 * scale * dir));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint lucide's `search` — a ring and a handle.
fn draw_search(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let radius = 7.0 * scale;
    // A rounded rect whose radius is half its own extent is a circle — the
    // vocabulary has no `Circle`, and building one out of arcs would be four
    // more curves to keep in sync with nothing.
    let ring = RoundedRect::new(
        centre.x - 2.0 * scale - radius,
        centre.y - 2.0 * scale - radius,
        centre.x - 2.0 * scale + radius,
        centre.y - 2.0 * scale + radius,
        radius,
    );
    scene.stroke_path(
        Point::ORIGIN,
        &Shape::to_path(&ring, style::PATH_TOLERANCE),
        LUCIDE_STROKE * scale,
        &Brush::Solid(color),
    );
    let mut handle = BezPath::new();
    handle.move_to(Point::new(3.5 * scale, 3.5 * scale));
    handle.line_to(Point::new(9.0 * scale, 9.0 * scale));
    scene.stroke_path(centre, &handle, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint lucide's `bell` — a dome over a clapper.
fn draw_bell(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    path.move_to(Point::new(-7.0 * scale, 5.0 * scale));
    path.line_to(Point::new(7.0 * scale, 5.0 * scale));
    path.move_to(Point::new(-5.0 * scale, 5.0 * scale));
    path.line_to(Point::new(-5.0 * scale, -2.0 * scale));
    path.line_to(Point::new(0.0, -7.0 * scale));
    path.line_to(Point::new(5.0 * scale, -2.0 * scale));
    path.line_to(Point::new(5.0 * scale, 5.0 * scale));
    path.move_to(Point::new(-2.0 * scale, 7.0 * scale));
    path.line_to(Point::new(2.0 * scale, 7.0 * scale));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint lucide's `eye` / `eye-off` — the privacy toggle's two marks.
fn draw_eye(scene: &mut dyn PaintScene, centre: Point, extent: f64, off: bool, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    path.move_to(Point::new(-10.0 * scale, 0.0));
    path.quad_to(Point::new(0.0, -8.0 * scale), Point::new(10.0 * scale, 0.0));
    path.quad_to(Point::new(0.0, 8.0 * scale), Point::new(-10.0 * scale, 0.0));
    if off {
        path.move_to(Point::new(-8.0 * scale, -8.0 * scale));
        path.line_to(Point::new(8.0 * scale, 8.0 * scale));
    }
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint lucide's `copy` / `check` — the copy affordance's two marks.
fn draw_copy(scene: &mut dyn PaintScene, centre: Point, extent: f64, copied: bool, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    if copied {
        path.move_to(Point::new(-5.0 * scale, 0.0));
        path.line_to(Point::new(-scale, 4.0 * scale));
        path.line_to(Point::new(6.0 * scale, -5.0 * scale));
    } else {
        path.move_to(Point::new(-2.0 * scale, -7.0 * scale));
        path.line_to(Point::new(7.0 * scale, -7.0 * scale));
        path.line_to(Point::new(7.0 * scale, 2.0 * scale));
        path.move_to(Point::new(-7.0 * scale, -2.0 * scale));
        path.line_to(Point::new(2.0 * scale, -2.0 * scale));
        path.line_to(Point::new(2.0 * scale, 7.0 * scale));
        path.line_to(Point::new(-7.0 * scale, 7.0 * scale));
        path.close_path();
    }
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint one primary action's mark: an arrow up, an arrow down to a line, a
/// repeat pair, or a card.
fn draw_action_mark(
    scene: &mut dyn PaintScene,
    centre: Point,
    extent: f64,
    action: WalletAction,
    color: Color,
) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    match action {
        WalletAction::Send => {
            path.move_to(Point::new(0.0, 7.0 * scale));
            path.line_to(Point::new(0.0, -7.0 * scale));
            path.move_to(Point::new(-5.0 * scale, -2.0 * scale));
            path.line_to(Point::new(0.0, -7.0 * scale));
            path.line_to(Point::new(5.0 * scale, -2.0 * scale));
        }
        WalletAction::Deposit => {
            path.move_to(Point::new(0.0, -7.0 * scale));
            path.line_to(Point::new(0.0, 3.0 * scale));
            path.move_to(Point::new(-5.0 * scale, -2.0 * scale));
            path.line_to(Point::new(0.0, 3.0 * scale));
            path.line_to(Point::new(5.0 * scale, -2.0 * scale));
            path.move_to(Point::new(-7.0 * scale, 7.0 * scale));
            path.line_to(Point::new(7.0 * scale, 7.0 * scale));
        }
        WalletAction::Swap => {
            path.move_to(Point::new(-7.0 * scale, -3.0 * scale));
            path.line_to(Point::new(7.0 * scale, -3.0 * scale));
            path.line_to(Point::new(3.0 * scale, -7.0 * scale));
            path.move_to(Point::new(7.0 * scale, 3.0 * scale));
            path.line_to(Point::new(-7.0 * scale, 3.0 * scale));
            path.line_to(Point::new(-3.0 * scale, 7.0 * scale));
        }
        WalletAction::Buy => {
            path.move_to(Point::new(-8.0 * scale, -5.0 * scale));
            path.line_to(Point::new(8.0 * scale, -5.0 * scale));
            path.line_to(Point::new(8.0 * scale, 5.0 * scale));
            path.line_to(Point::new(-8.0 * scale, 5.0 * scale));
            path.close_path();
            path.move_to(Point::new(-8.0 * scale, -scale));
            path.line_to(Point::new(8.0 * scale, -scale));
        }
    }
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(400.0, 700.0);

    // ---- Formatting ---------------------------------------------------------

    #[test]
    fn an_address_truncates_only_when_it_is_long_enough_to_need_it() {
        assert_eq!(truncate_address("0xabc"), "0xabc");
        // Twelve characters exactly is still shown whole.
        assert_eq!(truncate_address("0x1234567890"), "0x1234567890");
        assert_eq!(
            truncate_address("0x1234567890abcdef1234567890abcdef12345678"),
            "0x1234…5678"
        );
        // Counted in characters, not bytes: a multi-byte address is cut on a
        // character boundary rather than panicking mid-codepoint.
        let wide = "ααααααααααααααα";
        let cut = truncate_address(wide);
        assert_eq!(cut.chars().count(), 11);
        assert!(cut.contains('…'));
    }

    #[test]
    fn a_balance_is_grouped_and_always_carries_two_decimals() {
        assert_eq!(format_balance(0.0, "$"), "$0.00");
        assert_eq!(format_balance(1_234.5, "$"), "$1,234.50");
        assert_eq!(format_balance(1_234_567.891, "$"), "$1,234,567.89");
        assert_eq!(format_balance(999.0, "$"), "$999.00");
        assert_eq!(format_balance(-12.5, "$"), "$-12.50");
        // The prefix is the caller's, not a hardcoded dollar.
        assert_eq!(format_balance(5.0, "€"), "€5.00");
    }

    #[test]
    fn a_delta_carries_its_own_sign_and_never_a_negative_magnitude() {
        assert_eq!(format_delta(12.5), "+$12.50");
        assert_eq!(format_delta(-12.5), "-$12.50");
        assert_eq!(format_delta(0.0), "+$0.00");
        assert_eq!(format_delta(-2_500.0), "-$2,500.00");
        // The tone follows the sign, and zero reads as a fall — upstream's own
        // `> 0` test.
        assert_eq!(WalletDeltaTone::of(1.0), WalletDeltaTone::Up);
        assert_eq!(WalletDeltaTone::of(0.0), WalletDeltaTone::Down);
        assert_eq!(WalletDeltaTone::of(-1.0), WalletDeltaTone::Down);
    }

    #[test]
    fn an_account_takes_its_disc_from_its_own_first_character() {
        assert_eq!(wallet_account("a", "Main", "0x1").initial(), "M");
        assert_eq!(wallet_account("a", "élan", "0x1").initial(), "É");
        assert_eq!(wallet_account("a", "", "0x1").initial(), "");
    }

    // ---- The unread pulse ---------------------------------------------------

    #[test]
    fn the_unread_halo_grows_as_it_fades_and_then_restarts() {
        let (scale, alpha) = wallet_pulse(Duration::ZERO);
        assert_eq!(scale, 1.0);
        assert_eq!(alpha, 1.0);
        let (mid_scale, mid_alpha) = wallet_pulse(WALLET_PULSE_PERIOD.mul_f64(0.5));
        assert!(mid_scale > 1.0 && mid_scale < WALLET_PULSE_REACH);
        assert!(mid_alpha < 1.0 && mid_alpha > 0.0);
        let (late_scale, late_alpha) = wallet_pulse(WALLET_PULSE_PERIOD.mul_f64(0.99));
        assert!(late_scale > mid_scale);
        assert!(late_alpha < mid_alpha);
        // It cycles rather than settling.
        assert_eq!(
            wallet_pulse(WALLET_PULSE_PERIOD),
            wallet_pulse(Duration::ZERO)
        );
    }

    // ---- The mounted card ---------------------------------------------------

    #[derive(Default)]
    struct App {
        accounts_picked: Vec<String>,
        actions: Vec<WalletAction>,
        searches: Vec<String>,
        copies: Vec<String>,
        bells: u32,
        balance: f64,
        change: Option<f64>,
        notifications: bool,
    }

    fn accounts() -> Vec<WalletAccount> {
        vec![
            wallet_account("main", "Main", "0x1234567890abcdef1234567890abcdef12345678"),
            wallet_account(
                "cold",
                "Cold storage",
                "0xfedcba0987654321fedcba0987654321fedcba09",
            ),
        ]
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
                    balance: 12_345.67,
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
                let mut card = wallet_card(accounts(), s.balance)
                    .has_notifications(s.notifications)
                    .search_recent(vec!["ethereum".to_string(), "solana".to_string()])
                    .on_account_change(|s: &mut App, id| s.accounts_picked.push(id))
                    .on_action(|s: &mut App, action| s.actions.push(action))
                    .on_search_submit(|s: &mut App, term| s.searches.push(term))
                    .on_copy_address(|s: &mut App, address| s.copies.push(address))
                    .on_notifications(|s: &mut App| s.bells += 1);
                if let Some(change) = s.change {
                    card = card.change(change);
                }
                frust::stack().child(card)
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

        /// The trigger's centre, in window space.
        fn trigger(&mut self) -> Point {
            Point::new(
                WALLET_PADDING + 40.0,
                WALLET_PADDING + WALLET_HEAD_HEIGHT / 2.0,
            )
        }

        /// The search button's centre.
        fn search(&mut self) -> Point {
            let inner = WALLET_WIDTH - WALLET_PADDING * 2.0;
            let icons = WALLET_ICON_BUTTON * 2.0 + style::SPACING_UNIT;
            Point::new(
                WALLET_PADDING + inner - icons + WALLET_ICON_BUTTON / 2.0,
                WALLET_PADDING + WALLET_HEAD_HEIGHT / 2.0,
            )
        }

        /// The centre of the action at `index`.
        fn action(&mut self, index: usize) -> Point {
            let inner = WALLET_WIDTH - WALLET_PADDING * 2.0;
            let slot = inner / 4.0;
            let rec = self.step(0.0);
            let disc = rec
                .rrects
                .iter()
                .find(|(origin, size, _, _)| {
                    (size.width - WALLET_ACTION_SIZE).abs() < 0.001
                        && (origin.x
                            - (WALLET_PADDING
                                + slot * index as f64
                                + (slot - WALLET_ACTION_SIZE) / 2.0))
                            .abs()
                            < 0.001
                })
                .copied()
                .expect("an action disc");
            Rect::from_origin_size(disc.0, disc.1).center()
        }
    }

    #[test]
    fn the_account_trigger_opens_and_closes_the_switcher_panel() {
        let mut h = Harness::new();
        let at = h.trigger();
        h.click(at);
        // The panel is open and taller than the trigger it grew from.
        let rec = h.step(2_000.0);
        let panel = rec
            .rrects
            .iter()
            .find(|(_, size, _, _)| {
                size.height > WALLET_HEAD_HEIGHT + 1.0 && size.width < WALLET_WIDTH
            })
            .copied();
        assert!(panel.is_some(), "no panel grew from the trigger");
        // Pressing its own header shuts it again.
        h.click(at);
        h.step(2_000.0);
        let rec = h.step(0.0);
        assert!(
            rec.clips.is_empty(),
            "the panel is still clipping a list open"
        );
    }

    #[test]
    fn picking_an_account_reports_its_id_and_closes_the_panel() {
        let mut h = Harness::new();
        let at = h.trigger();
        h.click(at);
        h.step(2_000.0);
        // The second row: one row height below the first.
        let row = Point::new(
            WALLET_WIDTH / 2.0,
            WALLET_PADDING + WALLET_HEAD_HEIGHT + WALLET_LIST_PADDING + WALLET_ROW_HEIGHT * 1.5,
        );
        h.click(row);
        assert_eq!(h.state.accounts_picked, vec!["cold".to_string()]);
        h.step(2_000.0);
        let rec = h.step(0.0);
        assert!(rec.clips.is_empty(), "the panel stayed open after a pick");
    }

    #[test]
    fn the_copy_affordance_reports_the_full_address_not_the_truncated_one() {
        let mut h = Harness::new();
        let at = h.trigger();
        h.click(at);
        h.step(2_000.0);
        let copy = Point::new(
            WALLET_WIDTH - WALLET_PADDING - WALLET_ICON_BUTTON / 2.0,
            WALLET_PADDING + WALLET_HEAD_HEIGHT + WALLET_LIST_PADDING + WALLET_ROW_HEIGHT / 2.0,
        );
        h.click(copy);
        assert_eq!(
            h.state.copies,
            vec!["0x1234567890abcdef1234567890abcdef12345678".to_string()]
        );
        // ...and the panel is still open, because copying is not a selection.
        assert!(h.state.accounts_picked.is_empty());
    }

    #[test]
    fn the_search_trigger_opens_its_own_panel_and_a_pick_reports_the_term() {
        let mut h = Harness::new();
        let at = h.search();
        h.click(at);
        h.step(2_000.0);
        let row = Point::new(
            WALLET_WIDTH / 2.0,
            WALLET_PADDING + WALLET_HEAD_HEIGHT + WALLET_LIST_PADDING + WALLET_ROW_HEIGHT / 2.0,
        );
        h.click(row);
        assert_eq!(h.state.searches, vec!["ethereum".to_string()]);
    }

    #[test]
    fn only_one_panel_is_ever_open() {
        let mut h = Harness::new();
        let trigger = h.trigger();
        let search = h.search();
        h.click(trigger);
        h.step(2_000.0);
        // Pressing the search icon while the switcher is open: the switcher's
        // panel covers the header row, so the press lands on the panel and
        // closes it rather than opening a second one.
        h.click(search);
        h.step(2_000.0);
        let rec = h.step(0.0);
        assert!(rec.clips.is_empty(), "two panels ended up open at once");
    }

    #[test]
    fn each_primary_action_reports_itself() {
        let mut h = Harness::new();
        for (index, action) in WalletAction::ALL.into_iter().enumerate() {
            let at = h.action(index);
            h.click(at);
            assert_eq!(
                h.state.actions.last().copied(),
                Some(action),
                "action {index}"
            );
        }
        assert_eq!(h.state.actions.len(), 4);
    }

    #[test]
    fn the_privacy_toggle_masks_the_balance_and_hides_the_delta_with_it() {
        let mut h = Harness::new();
        h.state.change = Some(120.0);
        h.step(0.0);
        let shown = h.step(0.0).inks.len();
        // The eye sits just right of the "Balance" label, on the same line.
        let toggle = Point::new(
            WALLET_WIDTH / 2.0 + 30.0,
            WALLET_PADDING + WALLET_HEAD_HEIGHT + WALLET_BLOCK_GAP + 6.0,
        );
        h.click(toggle);
        let masked = h.step(2_000.0).inks.len();
        assert_ne!(shown, masked, "the balance did not swap for its mask");
    }

    #[test]
    fn escape_shuts_whichever_panel_is_open() {
        let mut h = Harness::new();
        let at = h.trigger();
        h.click(at);
        h.step(2_000.0);
        h.event(crate::components::popover::tests::escape());
        // Three frames: the first latches the exit's own clock, the second runs
        // it out, the third paints the settled card.
        h.step(0.0);
        h.step(2_000.0);
        let rec = h.step(0.0);
        assert!(rec.clips.is_empty(), "escape left the panel open");
    }

    #[test]
    fn a_reduced_motion_card_places_its_panel_at_once() {
        let mut h = Harness::themed(reduced());
        let at = h.trigger();
        h.click(at);
        let first = h.step(0.0);
        let second = h.step(16.0);
        assert_eq!(
            first.rrects, second.rrects,
            "a reduced-motion morph was still travelling"
        );
    }

    #[test]
    fn the_unread_bell_keeps_a_cosmetic_loop_running() {
        let mut h = Harness::new();
        h.state.notifications = true;
        h.step(0.0);
        let mut rec = Recorder::default();
        h.clock += 100.0;
        let now = h.clock;
        let outcome = h.root.paint(&mut rec, ft_ms(now));
        assert!(outcome.needs_frame, "the unread halo stopped pulsing");
    }

    // ---- The true-3D account fan --------------------------------------------

    /// A card built and laid out directly, with its account switcher already
    /// open — the route the 3D cases need, since the mounted harness hands back
    /// a scene rather than the widget behind it.
    #[cfg(feature = "gpu-effects")]
    fn opened(view: &WalletCardView<()>) -> WalletCardWidget {
        let mut next_id = 0u64;
        let mut widget = View::<()>::build(view, &mut frust_core::BuildCtx::new(&mut next_id));
        widget.set_panel(WalletPanel::Accounts);
        let mut tcx = TextContext::new();
        let mut layout = LayoutCtx::with_text_context(&mut tcx as &mut dyn Any);
        widget.layout(&mut layout, &BoxConstraints::tight(WINDOW));
        widget
    }

    /// A scene recording the two things the fan changes: the rounded rectangles
    /// the flat rows fill, and the scene textures composited.
    #[cfg(feature = "gpu-effects")]
    #[derive(Default)]
    struct FanScene {
        rrects: Vec<(Point, Size, f64, Color)>,
        textures: Vec<(u64, Rect)>,
    }

    #[cfg(feature = "gpu-effects")]
    impl PaintScene for FanScene {
        fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
        fn draw_text(&mut self, _origin: Point, _text: &str) {}
        fn fill_rounded_rect(&mut self, origin: Point, size: Size, radius: f64, color: Color) {
            self.rrects.push((origin, size, radius, color));
        }
        fn draw_scene_texture(&mut self, id: u64, dest: Rect) {
            self.textures.push((id, dest));
        }
    }

    /// The plates the fan submits: one per arrived row, in row order, the
    /// selected one at slot zero and nearest, and the run leaning away on both
    /// sides of it. Row order here happens to already be nearest-first for
    /// two accounts; `card3d::fan_scene`'s own sort is what makes that true in
    /// general rather than an accident of this fixture.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn the_account_fan_plates_every_arrived_row_in_row_order() {
        let widget = opened(&wallet_card(accounts(), 100.0).gpu_fan(true));
        let extent = (400, 400);

        let cards = widget.fan_cards(&[1.0, 1.0], Color::WHITE, extent);
        assert_eq!(cards.len(), 2, "both accounts get a plate");
        assert_eq!(cards[0].slot, 0, "the selected account is the stack's zero");
        assert_eq!(cards[1].slot, 1);
        // Each plate keeps its own row's box, mapped into the target.
        assert!((cards[0].dest.width() - widget.recent_rects[0].width()).abs() < 1e-9);
        assert!(cards[1].dest.y0 > cards[0].dest.y0);

        let scene = card3d::fan_scene(cards);
        assert!(scene.depth, "a stack occludes by distance");
        assert!(
            scene.quads[0].depth_offset > scene.quads[1].depth_offset,
            "the selected plate should sit nearest"
        );
        assert!(scene.quads[0].pitch.abs() < 1e-9);
        assert!(
            scene.quads[1].pitch > 0.0,
            "the row behind should lean away"
        );

        // A row that has not begun arriving contributes no plate at all.
        assert_eq!(widget.fan_cards(&[1.0, 0.0], Color::WHITE, extent).len(), 1);
        assert!(
            widget
                .fan_cards(&[0.0, 0.0], Color::WHITE, extent)
                .is_empty()
        );
    }

    /// The fallback contract: a test process installs no shell device, so the
    /// fan refuses and the panel paints *precisely* the 2D rows it always did —
    /// selected plate included, no scene texture composited.
    ///
    /// This is the path every caller of `gpu_fan` gets on Android, on iOS, and
    /// before a desktop shell's first surface, so it is the one the port is
    /// judged on rather than the enhancement above it.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn the_account_fan_degrades_to_the_two_dimensional_rows() {
        // Reduced motion collapses the morph, so one paint shows the panel
        // fully open with every row arrived — a deterministic comparison.
        let theme = reduced();
        let paint = |gpu: bool| {
            let mut widget = opened(&wallet_card(accounts(), 100.0).gpu_fan(gpu));
            let mut ctx = PaintCtx::for_test(Point::ORIGIN, WINDOW, ft_ms(0.0)).with_theme(&theme);
            let mut scene = FanScene::default();
            widget.paint(&mut ctx, &mut scene);
            (widget, scene)
        };

        let (widget, fanned) = paint(true);
        assert!(
            !widget.fan.is_active(),
            "a device-less run recorded a claim as held"
        );
        assert!(
            fanned.textures.is_empty(),
            "a device-less run composited a fan"
        );

        let (_, flat) = paint(false);
        assert_eq!(
            fanned.rrects, flat.rrects,
            "the opt-in changed the flat paint without a device"
        );
        assert!(
            !fanned.rrects.is_empty(),
            "the comparison painted no rounded rectangles at all"
        );
    }

    /// The fan's own claim, read back off real hardware with the *widget's*
    /// geometry rather than a synthetic stand-in: there are texels the card
    /// behind takes by paint order and depth hands back to the lifted selected
    /// card, even though the card behind is submitted after it.
    ///
    /// Counted rather than sampled at one coordinate, because where two lifted
    /// plates overlap is a consequence of the fan's own tuning and would make a
    /// fixed coordinate a test of the constants rather than of the occlusion.
    ///
    /// Ignored by default; see `crate::gpu_fx::test_gpu` for the invocation
    /// and why a read-back is the only honest check here.
    #[cfg(feature = "gpu-effects")]
    #[test]
    #[ignore = "needs a real GPU adapter"]
    fn the_account_fan_occludes_at_the_seam_by_depth() {
        use crate::gpu_fx::card3d::test_render::{pixel, render};
        use crate::gpu_fx::quad3d::Quad3dRenderer;
        use crate::gpu_fx::test_gpu::with_device;

        let widget = opened(&wallet_card(accounts(), 100.0).gpu_fan(true));
        let panel = widget.panel_box;
        let (_, extent) =
            card3d::target_for(panel.origin(), panel.size()).expect("the panel has a box");
        // Recoloured after the fact: the plates a wallet submits are all the
        // one muted tone, which cannot say *which* of two won a texel.
        let selected = Color::from_rgba8(255, 0, 0, 255);
        let behind = Color::from_rgba8(0, 0, 255, 255);
        let cards = || {
            let mut cards = widget.fan_cards(&[1.0, 1.0], Color::WHITE, extent);
            cards[0].face = QuadFace::Solid(selected);
            cards[1].face = QuadFace::Solid(behind);
            cards
        };

        // The selected row's own middle, where nothing contests it either way.
        let plate = card3d::in_target(
            extent,
            panel.size(),
            widget.recent_rects[0] - panel.origin().to_vec2(),
        );
        let (mid_x, mid_y) = (plate.center().x as u32, plate.center().y as u32);

        with_device(|device, queue| {
            let mut renderer = Quad3dRenderer::new(device, "wallet-fan");
            let occluded = render(
                device,
                queue,
                &mut renderer,
                extent,
                true,
                &card3d::fan_scene(cards()),
            );
            let painted = render(
                device,
                queue,
                &mut renderer,
                extent,
                false,
                &card3d::fan_scene(cards()).with_depth(false),
            );

            assert_eq!(
                pixel(&occluded, extent.0, mid_x, mid_y),
                [255, 0, 0, 255],
                "the selected plate did not render where it was laid out"
            );

            let contested = (0..extent.1)
                .flat_map(|y| (0..extent.0).map(move |x| (x, y)))
                .filter(|(x, y)| {
                    pixel(&painted, extent.0, *x, *y) == [0, 0, 255, 255]
                        && pixel(&occluded, extent.0, *x, *y) == [255, 0, 0, 255]
                })
                .count();
            println!("wallet fan: depth returned {contested} texels to the selected card");
            assert!(
                contested > 0,
                "paint order never handed the card behind a texel the lifted one should own"
            );
        });
    }

    /// Switching the mode off is a repaint, and re-declaring it is not.
    #[cfg(feature = "gpu-effects")]
    #[test]
    fn toggling_the_fan_repaints_and_redeclaring_it_does_not() {
        let mut next_id = 0u64;
        let mut ctx = frust_core::BuildCtx::new(&mut next_id);
        let on = wallet_card::<()>(accounts(), 100.0).gpu_fan(true);
        let mut widget = View::<()>::build(&on, &mut ctx);
        assert!(widget.gpu_fan);

        let again = wallet_card::<()>(accounts(), 100.0).gpu_fan(true);
        let flags = View::<()>::rebuild(&again, &on, &mut widget, &mut ctx);
        assert!(!flags.contains(ChangeFlags::PAINT));

        let off = wallet_card::<()>(accounts(), 100.0);
        let flags = View::<()>::rebuild(&off, &again, &mut widget, &mut ctx);
        assert!(flags.contains(ChangeFlags::PAINT));
        assert!(!widget.gpu_fan);
    }

    // ---- Typeface: the card and both panels follow the live theme ----------

    use crate::text::typeface_probe::{
        Face, Probe, assert_all, assert_control, assert_follows_a_live_family_swap,
        assert_follows_a_live_family_swap_on, assert_paints_only_in_geist,
    };

    /// The card with a balance change and one recent search term.
    fn probe_view(_: &mut ()) -> frust::StackView<()> {
        frust::stack().child(
            wallet_card::<()>(accounts(), 12_345.67)
                .change(2.5)
                .search_recent(vec!["ethereum".to_string()]),
        )
    }

    /// The account trigger's centre, in window space.
    const ACCOUNT_TRIGGER: Point = Point::new(
        WALLET_PADDING + 40.0,
        WALLET_PADDING + WALLET_HEAD_HEIGHT / 2.0,
    );

    /// The search button's centre, in window space.
    fn search_button() -> Point {
        let inner = WALLET_WIDTH - WALLET_PADDING * 2.0;
        let icons = WALLET_ICON_BUTTON * 2.0 + style::SPACING_UNIT;
        Point::new(
            WALLET_PADDING + inner - icons + WALLET_ICON_BUTTON / 2.0,
            WALLET_PADDING + WALLET_HEAD_HEIGHT / 2.0,
        )
    }

    /// A probe with the panel behind the head button at `at` opened by a press.
    fn opened_probe(
        at: Point,
    ) -> Probe<frust::StackView<()>, impl FnMut(&mut ()) -> frust::StackView<()>> {
        let mut probe = Probe::new(probe_view, WINDOW, crate::theme());
        probe.frame();
        probe.event(&pointer(PointerPhase::Down, at.x, at.y));
        probe.event(&pointer(PointerPhase::Up, at.x, at.y));
        probe
    }

    #[test]
    fn card_text_paints_in_geist_under_the_beui_theme() {
        assert_paints_only_in_geist("the card's text", probe_view, WINDOW);
        assert_control("the panels' text", WINDOW);
        let switcher = opened_probe(ACCOUNT_TRIGGER).frame();
        assert_all(
            "the account switcher",
            "under the beUI theme",
            &switcher,
            Face::Geist,
        );
        let search = opened_probe(search_button()).frame();
        assert_all(
            "the search panel",
            "under the beUI theme",
            &search,
            Face::Geist,
        );
        // The switcher adds each account's name, short address and initial; the
        // search panel adds its one recent term.
        let resting = Probe::new(probe_view, WINDOW, crate::theme()).frame();
        assert_eq!(switcher.len(), resting.len() + 6);
        assert_eq!(search.len(), resting.len() + 1);
    }

    #[test]
    fn card_text_follows_a_live_theme_family_swap() {
        assert_follows_a_live_family_swap("the card's text", probe_view, WINDOW);
        assert_follows_a_live_family_swap_on(
            "the account switcher",
            &mut opened_probe(ACCOUNT_TRIGGER),
        );
        assert_follows_a_live_family_swap_on(
            "the search panel",
            &mut opened_probe(search_button()),
        );
    }
}
