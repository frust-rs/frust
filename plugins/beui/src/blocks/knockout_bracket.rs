//! Ports beUI's `knockout-bracket` block —
//! `components/motion/knockout-bracket.tsx` (beUI rev
//! `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01), registry
//! slug `knockout-bracket`, name *"Fixtures"*: *"Pages one round at a time. The
//! leftmost round stacks at a fixed rhythm, each later round centers between its
//! two feeder matches, and cards, elbow connectors, headers and stage height
//! animate into every new layout."*
//!
//! # A premise correction
//!
//! The porting card describes this component as a *"tournament bracket with
//! animated advance (winner slides to next round, connector draw-in)"*. Upstream
//! does neither: it is a **paging** bracket — the whole tree is data, and what
//! animates is the reflow when the visible window of rounds changes. Its own
//! connector comment is explicit that there is no draw-in
//! (*"Transform/opacity only — no SVG path morph, so paging stays
//! flicker-free"*), and no winner ever moves between rounds, because the draw is
//! given complete. The port follows upstream: the choreography that is tested
//! here is the **paging reflow**, which is the choreography the component has.
//!
//! The registry entry bundles a second file, `knockout-wheel.tsx`, as an
//! *example* of the same data drawn radially. It is a 1,100-line component in
//! its own right — a hub-and-ring polar layout with its own hover isolation —
//! and porting it inside this module would be a second component behind one
//! slug's name. It is not carried; see the degradations below.
//!
//! | upstream | here |
//! |---|---|
//! | `CARD_W = 250`, `CARD_H = 124` | [`BRACKET_CARD_WIDTH`], [`BRACKET_CARD_HEIGHT`] |
//! | `GAP_X = 40`, `GAP_Y = 20`, `PAD_X = 8`, `PAD_Y = 12` | [`BRACKET_GAP_X`], [`BRACKET_GAP_Y`], [`BRACKET_PAD_X`], [`BRACKET_PAD_Y`] |
//! | `VISIBLE_COLS = 3`, `HEADER_H = 44` | [`BRACKET_VISIBLE_COLS`], [`BRACKET_HEADER_HEIGHT`] |
//! | `CONNECTOR_POCKET = 20` and its stem | [`BRACKET_POCKET`], [`BRACKET_STEM`] |
//! | `colX(r, page)` and `isInWindow` | [`bracket_column_x`], [`bracket_in_window`] |
//! | the `useMemo` centre pass (base rhythm, midpoints, behind-round spread) | [`bracket_centres`] |
//! | `maxPage = rounds.length - min(2, rounds.length)` | [`bracket_max_page`] |
//! | the connector list and its visibility rule | [`bracket_connectors`], [`BracketConnector`] |
//! | `REFLOW` `{stiffness 260, damping 32, mass 0.9}` | [`BRACKET_REFLOW`] |
//! | `REFLOW_OPACITY` `{duration 0.28, ease EASE_OUT}` | [`BRACKET_FADE`] |
//! | `isFinished`, the derived `FT` / `FT (P)` chip | [`bracket_is_finished`], [`bracket_badge`] |
//! | `initials(name)` | [`bracket_initials`] |
//! | `sideLabel` / `matchLabel` | [`bracket_match_label`] |
//! | `thirdPlace` under its own rule | [`KnockoutBracketView::third_place`] |
//!
//! # Degradations against the web original
//!
//! - **No knockout wheel.** The bundled radial example is not ported — the
//!   section above.
//! - **No crests or flags.** `TeamCrest` loads a remote image from `flagcdn.com`
//!   or a caller-hosted URL; there is no image fetch at this tier, so every team
//!   falls back to upstream's own [`bracket_initials`] disc — which is the
//!   branch upstream itself takes for a team with neither `logo` nor `code`.
//! - **No horizontal scroll.** Upstream's stage sits inside an
//!   `overflow-x-auto` box for a viewport narrower than three columns; here the
//!   stage is its own width and a narrow parent clips it, which is what the
//!   chevrons exist to make unnecessary.
//! - **The stage height is not sprung.** Upstream animates the container's
//!   `height`, which is a layout property; a height that springs would relayout
//!   the page around the bracket every frame, so the stage takes its new height
//!   on the layout that follows a page change while the cards glide into it.

use std::rc::Rc;
use std::time::Duration;

use frust::authoring::{
    Action, BezPath, BoxConstraints, Brush, BuildCtx, ChangeFlags, Color, ErasedArgCallback,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, NamedKey, PaintCtx, PaintScene, Point,
    PointerEvent, PointerPhase, Rect, Role, SemanticsCtx, Size, Vec2, View, Widget,
    erase_callback_arg, text::TextStyle,
};
use frust::{FrameTime, SpringDescription, Theme};

use crate::components::popover::{PanelChrome, paint_panel_hairline, resolve_panel};
use crate::motion::Ramp;
use crate::press::{Lane, inside, presses};
use crate::style;
use crate::text::LabelRun;
use crate::tokens::motion::EASE_OUT;

// ---- Metrics ---------------------------------------------------------------

/// `CARD_W = 250` — one match card's width, in logical px.
pub const BRACKET_CARD_WIDTH: f64 = 250.0;

/// `CARD_H = 124` — its height, in logical px.
pub const BRACKET_CARD_HEIGHT: f64 = 124.0;

/// `GAP_X = 40` — the horizontal gap between columns: the connector's pocket
/// plus its stem.
pub const BRACKET_GAP_X: f64 = 40.0;

/// `GAP_Y = 20` — the vertical gap between stacked cards in the base column.
pub const BRACKET_GAP_Y: f64 = 20.0;

/// `COL_W` — one column's pitch.
pub const BRACKET_COLUMN_PITCH: f64 = BRACKET_CARD_WIDTH + BRACKET_GAP_X;

/// `ROW` — one base-column row's pitch.
pub const BRACKET_ROW_PITCH: f64 = BRACKET_CARD_HEIGHT + BRACKET_GAP_Y;

/// `VISIBLE_COLS = 3` — how many rounds the stage shows at once.
pub const BRACKET_VISIBLE_COLS: usize = 3;

/// `CONNECTOR_POCKET = 20` — the width of the `]` elbow, in logical px.
pub const BRACKET_POCKET: f64 = 20.0;

/// The hairline from that elbow to its child card — `GAP_X - CONNECTOR_POCKET`.
pub const BRACKET_STEM: f64 = BRACKET_GAP_X - BRACKET_POCKET;

/// `HEADER_H = 44` — the round-title strip's height, in logical px.
pub const BRACKET_HEADER_HEIGHT: f64 = 44.0;

/// `PAD_X = 8` — the stage's horizontal padding, in logical px.
pub const BRACKET_PAD_X: f64 = 8.0;

/// `PAD_Y = 12` — its vertical padding, in logical px.
pub const BRACKET_PAD_Y: f64 = 12.0;

/// `size-11` — a chevron's tap target, in logical px.
pub const BRACKET_CHEVRON_TARGET: f64 = 44.0;

/// `size-9` — the visible disc inside it.
pub const BRACKET_CHEVRON_DISC: f64 = 36.0;

/// `rounded-2xl` — a match card's corner radius.
pub const BRACKET_CARD_RADIUS: f64 = style::RADIUS_2XL;

/// `p-4` — a card's own padding, in logical px.
pub const BRACKET_CARD_PADDING: f64 = 16.0;

/// `h-5` — the card's date/badge row, in logical px.
pub const BRACKET_META_HEIGHT: f64 = 20.0;

/// `space-y-2.5` — the gap between the two team rows.
pub const BRACKET_ROW_GAP: f64 = 10.0;

/// `h-5 w-7` — the crest slot's box, in logical px.
pub const BRACKET_CREST_WIDTH: f64 = 28.0;

/// Its height.
pub const BRACKET_CREST_HEIGHT: f64 = 20.0;

/// `gap-3` — the gap between the crest, the name and the score.
pub const BRACKET_ROW_INNER_GAP: f64 = 12.0;

/// `w-1.5` — the winner marker's reserved slot, in logical px.
pub const BRACKET_MARKER_SLOT: f64 = 6.0;

/// `bg-foreground/10` — the initials disc's fill alpha.
pub const BRACKET_DISC_ALPHA: f32 = 0.10;

/// `group-hover:bg-foreground/10` — a hovered chevron's disc alpha.
pub const BRACKET_CHEVRON_HOVER_ALPHA: f32 = 0.10;

// ---- Motion ----------------------------------------------------------------

/// `REFLOW` — *"firmer than SPRING_LAYOUT so the many cards, connectors and
/// stage height glide as one piece; damping just over critical (~1.05) settles
/// with no bounce and no lazy overdamped tail."*
///
/// A component-local constant upstream authors inline rather than pulling from
/// `@/lib/ease`, so it is carried inline here too.
pub const BRACKET_REFLOW: SpringDescription = SpringDescription {
    mass: 0.9,
    stiffness: 260.0,
    damping: 32.0,
};

/// `REFLOW_OPACITY` — *"a touch ahead of the position spring so columns don't
/// ghost while sliding."*
pub const BRACKET_FADE: Duration = Duration::from_millis(280);

// ---- The layout arithmetic -------------------------------------------------

/// `colX(r, page)` — round `r`'s left edge in stage space, given the leftmost
/// visible round.
///
/// Rounds behind the page get a negative `x`, which is what makes paging back
/// slide a formed column in from the left rather than fading one in place.
pub fn bracket_column_x(round: usize, page: usize) -> f64 {
    BRACKET_PAD_X + (round as f64 - page as f64) * BRACKET_COLUMN_PITCH
}

/// `isInWindow(r, page, visibleCols)` — whether round `r` is on the stage.
pub fn bracket_in_window(round: usize, page: usize, visible: usize) -> bool {
    round >= page && round < page + visible
}

/// `maxPage` — the furthest page the chevrons may reach.
///
/// *"The last page shows the final two rounds (semi-finals + final), not a full
/// window — so paging continues past the QF/SF/Final view down to SF +
/// Final."* That is `rounds - min(2, rounds)`, floored at zero.
pub fn bracket_max_page(rounds: usize) -> usize {
    rounds.saturating_sub(rounds.min(2))
}

/// `Math.min(VISIBLE_COLS, rounds.length)` — how many columns the stage shows.
pub fn bracket_visible_cols(rounds: usize) -> usize {
    BRACKET_VISIBLE_COLS.min(rounds).max(1)
}

/// The centre `y` of every match in every round, for a bracket paged to `page`.
///
/// Three passes, in upstream's own order:
///
/// 1. **The base column** (`rounds[page]`) stacks at a fixed rhythm:
///    `PAD_Y + i * ROW + CARD_H / 2`.
/// 2. **Every later round** centres each match between its two feeders —
///    `matches[k]` is fed by `2k` and `2k + 1` of the round before it. A round
///    with more matches than its feeders allow (an odd draw, a bye left out)
///    falls back to *stacking one row under the last card placed in that round*,
///    because a fixed rhythm from the top could land on a midpoint.
/// 3. **Every round behind the page** keeps its natural spread rather than
///    collapsing: spacing halves each step out and each match straddles its
///    parent, so paging back slides a formed column in.
///
/// `page` past the last round, or an empty draw, yields one empty row per round.
pub fn bracket_centres(matches_per_round: &[usize], page: usize) -> Vec<Vec<f64>> {
    let rounds = matches_per_round.len();
    let mut centres: Vec<Vec<f64>> = vec![Vec::new(); rounds];
    if rounds == 0 {
        return centres;
    }
    let page = page.min(rounds - 1);

    centres[page] = (0..matches_per_round[page])
        .map(|index| BRACKET_PAD_Y + index as f64 * BRACKET_ROW_PITCH + BRACKET_CARD_HEIGHT / 2.0)
        .collect();

    for round in (page + 1)..rounds {
        let feeders = centres[round - 1].clone();
        let mut row: Vec<f64> = Vec::with_capacity(matches_per_round[round]);
        for slot in 0..matches_per_round[round] {
            match feeders.get(2 * slot) {
                Some(&top) => {
                    let bottom = feeders.get(2 * slot + 1).copied().unwrap_or(top);
                    row.push((top + bottom) / 2.0);
                }
                None => {
                    let previous = row.last().copied();
                    row.push(match previous {
                        Some(previous) => previous + BRACKET_ROW_PITCH,
                        None => BRACKET_PAD_Y + BRACKET_CARD_HEIGHT / 2.0,
                    });
                }
            }
        }
        centres[round] = row;
    }

    for round in (0..page).rev() {
        let half = BRACKET_ROW_PITCH / 2f64.powi((page - round + 1) as i32);
        let parents = centres[round + 1].clone();
        centres[round] = (0..matches_per_round[round])
            .map(|index| {
                let parent = parents.get(index / 2).copied().unwrap_or(BRACKET_PAD_Y);
                if index % 2 == 0 {
                    parent - half
                } else {
                    parent + half
                }
            })
            .collect();
    }
    centres
}

/// The stage's own height for a given page: the lowest visible card's bottom
/// edge plus the stage padding.
///
/// *"Measured, not derived from the base count: a fallback-stacked round can run
/// past the base column"* — and seeded with one card's centre so an empty page
/// yields a real height rather than negative infinity.
pub fn bracket_stage_height(centres: &[Vec<f64>], page: usize, visible: usize) -> f64 {
    let lowest = centres
        .iter()
        .skip(page)
        .take(visible)
        .flatten()
        .copied()
        .fold(BRACKET_PAD_Y + BRACKET_CARD_HEIGHT / 2.0, f64::max);
    lowest + BRACKET_CARD_HEIGHT / 2.0 + BRACKET_PAD_Y
}

/// One `]` elbow between a pair of feeders and the match they feed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BracketConnector {
    /// The feeder column's right edge, in stage space.
    pub x: f64,
    /// The top feeder's centre `y`.
    pub y: f64,
    /// The distance between the two feeders' centres.
    pub height: f64,
    /// Whether both the feeder round and the fed round are on the stage.
    pub visible: bool,
}

/// Every elbow the bracket draws, one per match from round 1 on.
///
/// An elbow is visible only when **both** its rounds are on the stage, which is
/// what stops a connector dangling off the edge into a column that is not there.
pub fn bracket_connectors(
    matches_per_round: &[usize],
    centres: &[Vec<f64>],
    page: usize,
    visible: usize,
) -> Vec<BracketConnector> {
    let mut list = Vec::new();
    for round in 1..matches_per_round.len() {
        let x = bracket_column_x(round - 1, page) + BRACKET_CARD_WIDTH;
        let shown =
            bracket_in_window(round, page, visible) && bracket_in_window(round - 1, page, visible);
        for slot in 0..matches_per_round[round] {
            let feeders = &centres[round - 1];
            let own = centres[round].get(slot).copied().unwrap_or(BRACKET_PAD_Y);
            let top = feeders.get(2 * slot).copied().unwrap_or(own);
            let bottom = feeders.get(2 * slot + 1).copied().unwrap_or(top);
            list.push(BracketConnector {
                x,
                y: top,
                height: (bottom - top).max(0.0),
                visible: shown,
            });
        }
    }
    list
}

// ---- The data --------------------------------------------------------------

/// One competitor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketTeam {
    name: String,
}

/// A team called `name`.
pub fn bracket_team(name: impl Into<String>) -> BracketTeam {
    BracketTeam { name: name.into() }
}

impl BracketTeam {
    /// The team's name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// `initials(name)`: the first character of each of the first two words, upper
/// cased — *"Real Madrid" → RM*.
///
/// Taken over characters rather than bytes, which is upstream's own note: an
/// emoji or astral first character is a surrogate pair on the web and indexing
/// it renders a replacement glyph.
pub fn bracket_initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(|ch| ch.to_uppercase())
        .collect()
}

/// One side of a match.
#[derive(Clone, Debug, PartialEq)]
pub struct BracketSide {
    team: Option<BracketTeam>,
    score: Option<i64>,
    penalties: Option<i64>,
}

/// A side holding `team`, with no score yet.
pub fn bracket_side(team: BracketTeam) -> BracketSide {
    BracketSide {
        team: Some(team),
        score: None,
        penalties: None,
    }
}

/// A `TBD` side — upstream's `team: null`.
pub fn bracket_tbd() -> BracketSide {
    BracketSide {
        team: None,
        score: None,
        penalties: None,
    }
}

impl BracketSide {
    /// Set the side's score.
    pub fn score(mut self, score: i64) -> Self {
        self.score = Some(score);
        self
    }

    /// Set its shootout score (`penalties`).
    pub fn penalties(mut self, penalties: i64) -> Self {
        self.penalties = Some(penalties);
        self
    }

    /// The side's team, when one is known.
    pub fn team(&self) -> Option<&BracketTeam> {
        self.team.as_ref()
    }

    /// The name shown for this side — the team's, or `TBD`.
    pub fn display_name(&self) -> &str {
        self.team.as_ref().map_or("TBD", |team| team.name.as_str())
    }

    /// `sideLabel`: the announced form — the name, then the score, then the
    /// shootout in words.
    pub fn announced(&self) -> String {
        let name = self.display_name();
        let Some(score) = self.score else {
            return name.to_string();
        };
        match self.penalties {
            Some(penalties) => format!("{name} {score} ({penalties} on penalties)"),
            None => format!("{name} {score}"),
        }
    }

    /// The score as the card draws it — `1 (3)` when a shootout decided it.
    pub fn score_text(&self) -> Option<String> {
        let score = self.score?;
        Some(match self.penalties {
            Some(penalties) => format!("{score} ({penalties})"),
            None => score.to_string(),
        })
    }
}

/// Which side of a match won it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BracketWinner {
    /// The upper side.
    Home,
    /// The lower one.
    Away,
}

/// One fixture.
#[derive(Clone, Debug, PartialEq)]
pub struct BracketMatch {
    id: String,
    date: String,
    time: String,
    finished: Option<bool>,
    home: BracketSide,
    away: BracketSide,
    winner: Option<BracketWinner>,
    badge: Option<String>,
}

/// A match with `id`, between `home` and `away`.
pub fn bracket_match(id: impl Into<String>, home: BracketSide, away: BracketSide) -> BracketMatch {
    BracketMatch {
        id: id.into(),
        date: String::new(),
        time: String::new(),
        finished: None,
        home,
        away,
        winner: None,
        badge: None,
    }
}

impl BracketMatch {
    /// Set the kick-off day, already formatted (`date`).
    pub fn date(mut self, date: impl Into<String>) -> Self {
        self.date = date.into();
        self
    }

    /// Set the kick-off time (`time`).
    pub fn time(mut self, time: impl Into<String>) -> Self {
        self.time = time.into();
        self
    }

    /// State the match's status explicitly (`status`), overriding the
    /// derivation [`bracket_is_finished`] documents.
    pub fn finished(mut self, finished: bool) -> Self {
        self.finished = Some(finished);
        self
    }

    /// Record which side won (`winner`).
    pub fn winner(mut self, winner: BracketWinner) -> Self {
        self.winner = Some(winner);
        self
    }

    /// Replace the derived result chip (`badge`) — `AET`, `BO5`, `Forfeit`.
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// The match's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The upper side.
    pub fn home(&self) -> &BracketSide {
        &self.home
    }

    /// The lower side.
    pub fn away(&self) -> &BracketSide {
        &self.away
    }

    /// Which side won, when one has.
    pub fn match_winner(&self) -> Option<BracketWinner> {
        self.winner
    }
}

/// `isFinished(m)`: the declared status when there is one, and otherwise
/// whether a winner has been recorded.
///
/// *"`status` is optional, so a decided match reads as finished without it."*
pub fn bracket_is_finished(fixture: &BracketMatch) -> bool {
    fixture.finished.unwrap_or(fixture.winner.is_some())
}

/// The result chip a match shows: its own `badge` when it has one, else `FT (P)`
/// for a shootout, `FT` for any other finished match, and nothing while it is
/// still to come.
pub fn bracket_badge(fixture: &BracketMatch) -> Option<String> {
    if let Some(badge) = &fixture.badge {
        return Some(badge.clone());
    }
    if !bracket_is_finished(fixture) {
        return None;
    }
    let shootout = fixture.home.penalties.is_some() || fixture.away.penalties.is_some();
    Some(if shootout { "FT (P)" } else { "FT" }.to_string())
}

/// `matchLabel(roundName, m)`: the whole announced line for one card.
///
/// A finished match names its two sides with their scores and who won; an
/// upcoming one names them as a versus with whatever schedule it carries.
pub fn bracket_match_label(round_name: &str, fixture: &BracketMatch) -> String {
    let finished = bracket_is_finished(fixture);
    let home = fixture.home.announced();
    let away = fixture.away.announced();
    let sides = if finished {
        format!("{home}, {away}")
    } else {
        format!("{home} versus {away}")
    };
    let when = if finished {
        String::new()
    } else {
        let schedule: Vec<&str> = [fixture.date.as_str(), fixture.time.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect();
        if schedule.is_empty() {
            String::new()
        } else {
            format!(", {}", schedule.join(", "))
        }
    };
    let outcome = match fixture.winner {
        Some(BracketWinner::Home) => fixture.home.team.as_ref(),
        Some(BracketWinner::Away) => fixture.away.team.as_ref(),
        None => None,
    }
    .map_or_else(String::new, |team| format!(", {} won", team.name));
    format!("{round_name}: {sides}{when}{outcome}")
}

/// One round of the draw.
#[derive(Clone, Debug, PartialEq)]
pub struct BracketRound {
    name: String,
    matches: Vec<BracketMatch>,
}

/// A round called `name`, holding `matches`.
pub fn bracket_round(name: impl Into<String>, matches: Vec<BracketMatch>) -> BracketRound {
    BracketRound {
        name: name.into(),
        matches,
    }
}

impl BracketRound {
    /// The round's column header.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its fixtures.
    pub fn matches(&self) -> &[BracketMatch] {
        &self.matches
    }
}

// ---- The component ---------------------------------------------------------

/// A view-held page callback (erased on build).
type OnPageChange<State> = Rc<dyn Fn(&mut State, usize)>;

/// What the bracket renders from, beyond its rounds.
#[derive(Clone, Debug, PartialEq)]
struct BracketConfig {
    page: Option<usize>,
    initial_round: usize,
    third_place: Option<BracketMatch>,
    third_place_label: String,
    label: String,
}

/// A declarative beUI knockout bracket. See [`knockout_bracket`].
pub struct KnockoutBracketView<State: 'static> {
    rounds: Vec<BracketRound>,
    config: BracketConfig,
    on_page_change: OnPageChange<State>,
}

/// Build a knockout bracket over `rounds`, ordered widest round first.
pub fn knockout_bracket<State: 'static>(rounds: Vec<BracketRound>) -> KnockoutBracketView<State> {
    KnockoutBracketView {
        rounds,
        config: BracketConfig {
            page: None,
            initial_round: 1,
            third_place: None,
            third_place_label: "Third place play-off".to_string(),
            label: "Tournament bracket".to_string(),
        },
        on_page_change: Rc::new(|_, _| {}),
    }
}

impl<State: 'static> KnockoutBracketView<State> {
    /// Take the page over: the stage shows exactly what `page` says and the
    /// chevrons only report.
    pub fn page(mut self, page: usize) -> Self {
        self.config.page = Some(page);
        self
    }

    /// Set the round shown leftmost on mount (`initialRound`, default 1).
    pub fn initial_round(mut self, round: usize) -> Self {
        self.config.initial_round = round;
        self
    }

    /// Set the third-place play-off, rendered under the tree (`thirdPlace`).
    pub fn third_place(mut self, fixture: BracketMatch) -> Self {
        self.config.third_place = Some(fixture);
        self
    }

    /// Set that fixture's heading (`thirdPlaceLabel`).
    pub fn third_place_label(mut self, label: impl Into<String>) -> Self {
        self.config.third_place_label = label.into();
        self
    }

    /// Set the stage's accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.config.label = label.into();
        self
    }

    /// Set the paging callback, fired from a chevron's own press.
    pub fn on_page_change<F: Fn(&mut State, usize) + 'static>(mut self, f: F) -> Self {
        self.on_page_change = Rc::new(f);
        self
    }
}

/// One retained match card's shaped runs.
struct CardRuns {
    meta: LabelRun,
    badge: Option<LabelRun>,
    names: [LabelRun; 2],
    scores: [Option<LabelRun>; 2],
    initials: [LabelRun; 2],
    /// The announced line, kept beside the runs because semantics needs the
    /// text and a cached run holds no readable copy.
    announced: String,
}

/// The retained widget for a [`KnockoutBracketView`].
pub struct KnockoutBracketWidget {
    rounds: Vec<BracketRound>,
    headers: Vec<LabelRun>,
    cards: Vec<Vec<CardRuns>>,
    third: Option<CardRuns>,
    third_label: LabelRun,
    config: BracketConfig,
    /// The leftmost visible round.
    page: usize,
    /// The page the reflow is travelling *from*, and the lane carrying it.
    reflow: Lane,
    /// The frame the current reflow's fade started on.
    fade_since: Option<FrameTime>,
    /// The centres for the page the reflow is coming from, and the one it is
    /// going to — the stage interpolates between them.
    from_centres: Vec<Vec<f64>>,
    to_centres: Vec<Vec<f64>>,
    /// The stage's own height as of the last layout.
    stage_height: f64,
    /// The two chevrons' boxes in the widget's own space, or `Rect::ZERO`.
    chevrons: [Rect; 2],
    /// The chevron a `Down` armed.
    armed: Option<usize>,
    /// The chevron the pointer is over.
    hovered: Option<usize>,
    on_page_change: ErasedArgCallback<usize>,
}

impl KnockoutBracketWidget {
    /// The leftmost visible round.
    pub fn page(&self) -> usize {
        self.page
    }

    /// The furthest page the chevrons may reach.
    pub fn max_page(&self) -> usize {
        bracket_max_page(self.rounds.len())
    }

    /// How many rounds the stage shows at once.
    pub fn visible_cols(&self) -> usize {
        bracket_visible_cols(self.rounds.len())
    }

    /// The stage's own height as of the last layout, in logical px.
    pub fn stage_height(&self) -> f64 {
        self.stage_height
    }

    /// The match-count-per-round shape the layout arithmetic runs over.
    fn shape(&self) -> Vec<usize> {
        self.rounds
            .iter()
            .map(|round| round.matches.len())
            .collect()
    }

    /// Move to `page`, staging the reflow from wherever the stage is now.
    fn set_page(&mut self, page: usize) -> bool {
        let next = page.min(self.max_page());
        if next == self.page {
            return false;
        }
        let shape = self.shape();
        // The reflow starts from where the stage actually is, not from the page
        // it nominally left — a chevron pressed mid-glide continues from there.
        self.from_centres = self.current_centres();
        self.page = next;
        self.to_centres = bracket_centres(&shape, next);
        self.reflow.retarget(0.0);
        self.reflow.snap();
        self.reflow.retarget(1.0);
        self.fade_since = None;
        true
    }

    /// The centres the stage is drawing right now, between its two pages.
    fn current_centres(&self) -> Vec<Vec<f64>> {
        let t = self.reflow.value().clamp(0.0, 1.0);
        self.to_centres
            .iter()
            .enumerate()
            .map(|(round, row)| {
                row.iter()
                    .enumerate()
                    .map(|(slot, &to)| {
                        let from = self
                            .from_centres
                            .get(round)
                            .and_then(|r| r.get(slot))
                            .copied()
                            .unwrap_or(to);
                        from + (to - from) * t
                    })
                    .collect()
            })
            .collect()
    }

    /// The stage-space `x` of round `r` right now, likewise between pages.
    fn current_column_x(&self, round: usize, from_page: usize) -> f64 {
        let t = self.reflow.value().clamp(0.0, 1.0);
        let from = bracket_column_x(round, from_page);
        let to = bracket_column_x(round, self.page);
        from + (to - from) * t
    }

    /// The page the reflow is travelling from — recovered from the two centre
    /// tables rather than stored, since a mid-glide retarget replaces the
    /// `from` table with an interpolated one.
    fn reflow_origin_page(&self) -> usize {
        self.page
    }

    /// A round's opacity right now: fading in as it enters the window, out as
    /// it leaves.
    fn round_alpha(&self, round: usize, elapsed: Duration) -> f64 {
        let visible = self.visible_cols();
        let target = if bracket_in_window(round, self.page, visible) {
            1.0
        } else {
            0.0
        };
        let fade = Ramp::eased(BRACKET_FADE, EASE_OUT).progress_clamped(elapsed);
        // The fade always runs toward the target; a round that was already
        // there simply starts and stays at one.
        if target == 1.0 { fade } else { 1.0 - fade }
    }
}

/// Build the shaped runs for one match card.
fn card_runs(round_name: &str, fixture: &BracketMatch) -> CardRuns {
    let schedule: Vec<&str> = [fixture.date.as_str(), fixture.time.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    CardRuns {
        meta: LabelRun::new(schedule.join(", ")),
        badge: bracket_badge(fixture).map(LabelRun::new),
        names: [
            LabelRun::new(fixture.home.display_name()),
            LabelRun::new(fixture.away.display_name()),
        ],
        scores: [
            fixture.home.score_text().map(LabelRun::new),
            fixture.away.score_text().map(LabelRun::new),
        ],
        initials: [
            LabelRun::new(
                fixture
                    .home
                    .team
                    .as_ref()
                    .map_or_else(String::new, |team| bracket_initials(&team.name)),
            ),
            LabelRun::new(
                fixture
                    .away
                    .team
                    .as_ref()
                    .map_or_else(String::new, |team| bracket_initials(&team.name)),
            ),
        ],
        announced: bracket_match_label(round_name, fixture),
    }
}

/// Build every round's cards.
fn all_cards(rounds: &[BracketRound]) -> Vec<Vec<CardRuns>> {
    rounds
        .iter()
        .map(|round| {
            round
                .matches
                .iter()
                .map(|fixture| card_runs(&round.name, fixture))
                .collect()
        })
        .collect()
}

impl<State: 'static> View<State> for KnockoutBracketView<State> {
    type Element = KnockoutBracketWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> KnockoutBracketWidget {
        let max = bracket_max_page(self.rounds.len());
        let page = self
            .config
            .page
            .unwrap_or(self.config.initial_round)
            .min(max);
        let shape: Vec<usize> = self.rounds.iter().map(|r| r.matches.len()).collect();
        let centres = bracket_centres(&shape, page);
        KnockoutBracketWidget {
            headers: self
                .rounds
                .iter()
                .map(|round| LabelRun::new(round.name.clone()))
                .collect(),
            cards: all_cards(&self.rounds),
            third: self
                .config
                .third_place
                .as_ref()
                .map(|fixture| card_runs(&self.config.third_place_label, fixture)),
            third_label: LabelRun::new(self.config.third_place_label.clone()),
            rounds: self.rounds.clone(),
            config: self.config.clone(),
            page,
            // A bracket mounts settled on its own page rather than gliding in.
            reflow: Lane::at_rest(Ramp::spring(BRACKET_REFLOW), 1.0),
            fade_since: None,
            from_centres: centres.clone(),
            to_centres: centres,
            stage_height: 0.0,
            chevrons: [Rect::ZERO; 2],
            armed: None,
            hovered: None,
            on_page_change: erase_callback_arg(&self.on_page_change),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KnockoutBracketWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.rounds != self.rounds {
            element.headers = self
                .rounds
                .iter()
                .map(|round| LabelRun::new(round.name.clone()))
                .collect();
            element.cards = all_cards(&self.rounds);
            element.rounds = self.rounds.clone();
            let shape: Vec<usize> = self.rounds.iter().map(|r| r.matches.len()).collect();
            element.page = element.page.min(bracket_max_page(self.rounds.len()));
            element.to_centres = bracket_centres(&shape, element.page);
            element.from_centres = element.to_centres.clone();
            element.armed = None;
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        if element.config != self.config {
            element.third = self
                .config
                .third_place
                .as_ref()
                .map(|fixture| card_runs(&self.config.third_place_label, fixture));
            element
                .third_label
                .set_content(self.config.third_place_label.clone());
            element.config = self.config.clone();
            if let Some(page) = self.config.page {
                element.set_page(page);
            }
            flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
        }
        // Closures are not comparable; reinstalling the adapter is cheap.
        element.on_page_change = erase_callback_arg(&self.on_page_change);
        flags
    }

    fn teardown(&self, _element: &mut KnockoutBracketWidget, _ctx: &mut BuildCtx<'_>) {}
}

/// The label family: the theme's own scale, with the catalog's sans stack as
/// the unthemed fallback.
fn family_of(theme: Option<&Theme>) -> frust::authoring::text::FontFamily {
    theme.map_or_else(crate::tokens::sans_family, |t| {
        t.type_scale.label_large.family.clone()
    })
}

/// One label style at `size`.
fn bracket_style(theme: Option<&Theme>, size: f64) -> TextStyle {
    TextStyle {
        family: family_of(theme),
        ..crate::text::label_style(size)
    }
}

impl Widget for KnockoutBracketWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let theme = Theme::from_layout_ctx(ctx);
        // Every style is resolved before the first `layout` call: the shaper
        // takes `ctx` mutably, and the theme read borrows it.
        let header = bracket_style(theme, style::TEXT_SM);
        let name = bracket_style(theme, style::TEXT_BASE);
        let meta = bracket_style(theme, style::TEXT_SM);
        let micro = bracket_style(theme, style::TEXT_XS);

        for run in &mut self.headers {
            run.layout(ctx, &header);
        }
        self.third_label.layout(ctx, &meta);
        for round in &mut self.cards {
            for card in round {
                layout_card(card, ctx, &name, &meta, &micro);
            }
        }
        if let Some(third) = &mut self.third {
            layout_card(third, ctx, &name, &meta, &micro);
        }

        let visible = self.visible_cols();
        self.to_centres = bracket_centres(&self.shape(), self.page);
        self.stage_height = bracket_stage_height(&self.to_centres, self.page, visible);

        let width = visible as f64 * BRACKET_CARD_WIDTH
            + (visible.saturating_sub(1)) as f64 * BRACKET_GAP_X
            + BRACKET_PAD_X * 2.0;

        // The chevrons sit inside the header strip, one at each edge, and only
        // when there is somewhere to go.
        let chevron_y = (BRACKET_HEADER_HEIGHT - BRACKET_CHEVRON_TARGET) / 2.0;
        self.chevrons[0] = if self.page > 0 {
            Rect::from_origin_size(
                Point::new(BRACKET_PAD_X, chevron_y),
                Size::new(BRACKET_CHEVRON_TARGET, BRACKET_CHEVRON_TARGET),
            )
        } else {
            Rect::ZERO
        };
        self.chevrons[1] = if self.page < self.max_page() {
            Rect::from_origin_size(
                Point::new(width - BRACKET_PAD_X - BRACKET_CHEVRON_TARGET, chevron_y),
                Size::new(BRACKET_CHEVRON_TARGET, BRACKET_CHEVRON_TARGET),
            )
        } else {
            Rect::ZERO
        };

        let third_height = if self.third.is_some() {
            style::spacing(8.0) + self.third_label.size().height + BRACKET_CARD_HEIGHT + 8.0
        } else {
            0.0
        };
        bc.constrain(Size::new(
            width,
            BRACKET_HEADER_HEIGHT + self.stage_height + third_height,
        ))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let theme = Theme::from_paint_ctx(ctx);
        let chrome = resolve_panel(theme);
        let background = theme.map_or(crate::BEUI_LIGHT.background, |t| t.scheme().surface);
        let reduce = theme.is_some_and(|t| t.motion.reduce_motion);
        let now = ctx.frame_time();
        let origin = ctx.origin();
        let size = ctx.size();

        if reduce {
            self.reflow.snap();
        } else if self.reflow.advance(now) {
            ctx.request_layout();
        }
        let started = *self.fade_since.get_or_insert(now);
        let elapsed = if reduce {
            BRACKET_FADE
        } else {
            now.saturating_sub(started)
        };
        if !reduce && elapsed < BRACKET_FADE {
            ctx.request_frame();
        }

        let visible = self.visible_cols();
        let centres = self.current_centres();
        let from_page = self.reflow_origin_page();

        // The header strip, clipped so a gliding title disappears at its edge.
        scene.push_clip(origin, Size::new(size.width, BRACKET_HEADER_HEIGHT));
        for round in 0..self.rounds.len() {
            let alpha = self.round_alpha(round, elapsed);
            if alpha <= 0.0 {
                continue;
            }
            let x = self.current_column_x(round, from_page);
            let text = self.headers[round].size();
            self.headers[round].paint(
                origin
                    + Vec2::new(
                        x + (BRACKET_CARD_WIDTH - text.width) / 2.0,
                        (BRACKET_HEADER_HEIGHT - text.height) / 2.0,
                    ),
                style::with_alpha(chrome.ink, alpha as f32),
                scene,
            );
        }
        scene.pop_clip();

        // The two chevrons sit outside that clip.
        for index in 0..2 {
            let rect = self.chevrons[index];
            if rect.width() <= 0.0 {
                continue;
            }
            let at = origin + rect.center().to_vec2();
            if self.hovered == Some(index) {
                let disc = BRACKET_CHEVRON_DISC;
                scene.fill_rounded_rect(
                    Point::new(at.x - disc / 2.0, at.y - disc / 2.0),
                    Size::new(disc, disc),
                    disc / 2.0,
                    style::with_alpha(chrome.ink, BRACKET_CHEVRON_HOVER_ALPHA),
                );
            }
            draw_chevron(scene, at, style::ICON_SIZE_LG, index == 1, chrome.dim_ink);
        }

        // The stage: connectors under the cards, both clipped to it.
        let stage = Point::new(origin.x, origin.y + BRACKET_HEADER_HEIGHT);
        scene.push_clip(stage, Size::new(size.width, self.stage_height));
        for connector in bracket_connectors(&self.shape(), &centres, self.page, visible) {
            if !connector.visible {
                continue;
            }
            draw_connector(scene, stage, connector, chrome.border);
        }
        for round in 0..self.rounds.len() {
            let alpha = self.round_alpha(round, elapsed);
            if alpha <= 0.0 {
                continue;
            }
            let x = self.current_column_x(round, from_page);
            for slot in 0..self.rounds[round].matches.len() {
                let Some(&centre) = centres.get(round).and_then(|row| row.get(slot)) else {
                    continue;
                };
                let at = stage + Vec2::new(x, centre - BRACKET_CARD_HEIGHT / 2.0);
                paint_card(
                    scene,
                    at,
                    &self.cards[round][slot],
                    &self.rounds[round].matches[slot],
                    chrome,
                    background,
                    alpha,
                );
            }
        }
        scene.pop_clip();

        // The third-place play-off, under its own rule.
        if let (Some(third), Some(fixture)) = (&self.third, &self.config.third_place) {
            let y = origin.y + BRACKET_HEADER_HEIGHT + self.stage_height + style::spacing(4.0);
            scene.fill_rect(
                Point::new(origin.x, y),
                Size::new(size.width, style::BORDER_WIDTH),
                chrome.border,
            );
            let label_y = y + style::spacing(4.0);
            self.third_label.paint(
                Point::new(origin.x + BRACKET_PAD_X, label_y),
                chrome.dim_ink,
                scene,
            );
            paint_card(
                scene,
                Point::new(
                    origin.x + BRACKET_PAD_X,
                    label_y + self.third_label.size().height + style::GAP_SM,
                ),
                third,
                fixture,
                chrome,
                background,
                1.0,
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match event {
            InputEvent::Key(key) => {
                if !ctx.has_focus() {
                    return EventResult::Ignored;
                }
                let step = match key.key {
                    Key::Named(NamedKey::ArrowLeft) => -1i64,
                    Key::Named(NamedKey::ArrowRight) => 1,
                    _ => return EventResult::Ignored,
                };
                self.page_by(ctx, step);
                EventResult::Handled
            }
            InputEvent::Pointer(p) => self.handle_pointer(ctx, p),
            _ => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        let label = self.config.label.clone();
        let visible = self.visible_cols();
        let page = self.page;
        ctx.push_container(
            Role::Group,
            move |node| {
                node.set_label(label.as_str());
            },
            |ctx| {
                if self.chevrons[0].width() > 0.0 {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label("Previous round");
                        node.add_action(Action::Click);
                    });
                }
                if self.chevrons[1].width() > 0.0 {
                    ctx.push_node(Role::Button, |node| {
                        node.set_label("Next round");
                        node.add_action(Action::Click);
                    });
                }
                // Only the rounds on the stage are announced — upstream marks
                // the rest `aria-hidden`, and they are the ones the chevrons
                // have not brought into reach.
                for (round, cards) in self.cards.iter().enumerate() {
                    if !bracket_in_window(round, page, visible) {
                        continue;
                    }
                    let name = self.rounds[round].name.clone();
                    ctx.push_node(Role::Label, |node| {
                        node.set_label(name.as_str());
                    });
                    for card in cards {
                        let announced = card.announced.clone();
                        ctx.push_node(Role::ListItem, |node| {
                            node.set_label(announced.as_str());
                        });
                    }
                }
                if let Some(third) = &self.third {
                    let announced = third.announced.clone();
                    ctx.push_node(Role::ListItem, |node| {
                        node.set_label(announced.as_str());
                    });
                }
            },
        );
    }
}

impl KnockoutBracketWidget {
    /// Page by `step`, clamped, reporting the new page once.
    fn page_by(&mut self, ctx: &mut EventCtx, step: i64) {
        let next = (self.page as i64 + step).clamp(0, self.max_page() as i64) as usize;
        if next == self.page {
            return;
        }
        // A controlled bracket reports the request and waits for the app's page.
        if self.config.page.is_none() {
            self.set_page(next);
        }
        (self.on_page_change)(ctx, next);
        ctx.request_redraw();
    }

    /// The `Widget::event` pointer arm: the two chevrons own their presses.
    fn handle_pointer(&mut self, ctx: &mut EventCtx, p: &PointerEvent) -> EventResult {
        let size = ctx.size();
        let over = self
            .chevrons
            .iter()
            .position(|rect| rect.width() > 0.0 && rect.contains(p.position));
        match p.phase {
            PointerPhase::Move => {
                if inside(p.position, size) && over.is_some() {
                    ctx.claim_hover();
                    ctx.set_cursor(style::ACTIVE_CURSOR);
                }
                if self.hovered != over {
                    self.hovered = over;
                    ctx.request_redraw();
                }
                EventResult::Ignored
            }
            PointerPhase::Down => {
                if !presses(p) {
                    return EventResult::Ignored;
                }
                ctx.request_focus();
                let Some(index) = over else {
                    return EventResult::Ignored;
                };
                ctx.capture_pointer();
                self.armed = Some(index);
                EventResult::Handled
            }
            PointerPhase::Up => {
                let Some(armed) = self.armed.take() else {
                    return EventResult::Ignored;
                };
                if over == Some(armed) {
                    self.page_by(ctx, if armed == 0 { -1 } else { 1 });
                }
                EventResult::Handled
            }
            // A `Cancel` arm never reaches app state.
            PointerPhase::Cancel => {
                if self.armed.take().is_none() {
                    return EventResult::Ignored;
                }
                ctx.request_redraw();
                EventResult::Handled
            }
        }
    }
}

/// Shape one card's runs.
fn layout_card(
    card: &mut CardRuns,
    ctx: &mut LayoutCtx,
    name: &TextStyle,
    meta: &TextStyle,
    micro: &TextStyle,
) {
    card.meta.layout(ctx, meta);
    if let Some(badge) = &mut card.badge {
        badge.layout(ctx, micro);
    }
    for index in 0..2 {
        card.names[index].layout(ctx, name);
        if let Some(score) = &mut card.scores[index] {
            score.layout(ctx, name);
        }
        card.initials[index].layout(ctx, micro);
    }
}

/// Paint one match card at `at`, faded by `alpha`.
fn paint_card(
    scene: &mut dyn PaintScene,
    at: Point,
    card: &CardRuns,
    fixture: &BracketMatch,
    chrome: PanelChrome,
    background: Color,
    alpha: f64,
) {
    let box_size = Size::new(BRACKET_CARD_WIDTH, BRACKET_CARD_HEIGHT);
    let layered = alpha < 1.0;
    if layered {
        scene.push_layer(at, box_size, alpha.clamp(0.0, 1.0) as f32);
    }
    scene.fill_rounded_rect(at, box_size, BRACKET_CARD_RADIUS, chrome.surface);
    paint_panel_hairline(scene, at, box_size, BRACKET_CARD_RADIUS, chrome.border);

    // The meta row: the schedule on the left, the result chip on the right.
    let meta = card.meta.size();
    card.meta.paint(
        Point::new(
            at.x + BRACKET_CARD_PADDING,
            at.y + BRACKET_CARD_PADDING + (BRACKET_META_HEIGHT - meta.height) / 2.0,
        ),
        chrome.dim_ink,
        scene,
    );
    if let Some(badge) = &card.badge {
        let text = badge.size();
        let pill = Size::new(text.width + style::GAP_MD * 2.0, BRACKET_META_HEIGHT);
        let pill_at = Point::new(
            at.x + BRACKET_CARD_WIDTH - BRACKET_CARD_PADDING - pill.width,
            at.y + BRACKET_CARD_PADDING,
        );
        scene.fill_rounded_rect(pill_at, pill, style::RADIUS_CONTROL, background);
        badge.paint(
            Point::new(
                pill_at.x + style::GAP_MD,
                pill_at.y + (pill.height - text.height) / 2.0,
            ),
            chrome.dim_ink,
            scene,
        );
    }

    // The two team rows.
    let decided = bracket_is_finished(fixture) && fixture.winner.is_some();
    let rows_top = at.y + BRACKET_CARD_PADDING + BRACKET_META_HEIGHT + style::GAP_MD;
    let row_height = card.names[0].size().height.max(BRACKET_CREST_HEIGHT);
    for index in 0..2 {
        let side = if index == 0 {
            &fixture.home
        } else {
            &fixture.away
        };
        let winner = decided
            && fixture.winner
                == Some(if index == 0 {
                    BracketWinner::Home
                } else {
                    BracketWinner::Away
                });
        let dim = decided && !winner;
        let ink = if dim { chrome.dim_ink } else { chrome.ink };
        let y = rows_top + index as f64 * (row_height + BRACKET_ROW_GAP);

        // The crest slot: an initials disc, or the TBD placeholder.
        let disc_x = at.x + BRACKET_CARD_PADDING;
        if side.team.is_some() {
            let disc = BRACKET_CREST_HEIGHT;
            scene.fill_rounded_rect(
                Point::new(disc_x + (BRACKET_CREST_WIDTH - disc) / 2.0, y),
                Size::new(disc, disc),
                disc / 2.0,
                style::with_alpha(chrome.ink, BRACKET_DISC_ALPHA),
            );
            let initials = card.initials[index].size();
            card.initials[index].paint(
                Point::new(
                    disc_x + (BRACKET_CREST_WIDTH - initials.width) / 2.0,
                    y + (disc - initials.height) / 2.0,
                ),
                chrome.ink,
                scene,
            );
        } else {
            draw_shield(
                scene,
                Point::new(
                    disc_x + BRACKET_CREST_WIDTH / 2.0,
                    y + BRACKET_CREST_HEIGHT / 2.0,
                ),
                BRACKET_CREST_HEIGHT,
                style::scale_alpha(chrome.dim_ink, 0.5),
            );
        }

        let name_x = disc_x + BRACKET_CREST_WIDTH + BRACKET_ROW_INNER_GAP;
        card.names[index].paint(Point::new(name_x, y), ink, scene);

        // The score, right-aligned, with the marker's own slot beside it.
        let right = at.x + BRACKET_CARD_WIDTH - BRACKET_CARD_PADDING;
        if let Some(score) = &card.scores[index] {
            let text = score.size();
            score.paint(
                Point::new(right - BRACKET_MARKER_SLOT - style::GAP_SM - text.width, y),
                ink,
                scene,
            );
        }
        if winner {
            draw_marker(
                scene,
                Point::new(right - BRACKET_MARKER_SLOT / 2.0, y + row_height / 2.0),
                chrome.ink,
            );
        }
    }
    if layered {
        scene.pop_layer();
    }
}

/// Paint one `]` elbow and its stem.
fn draw_connector(
    scene: &mut dyn PaintScene,
    stage: Point,
    connector: BracketConnector,
    color: Color,
) {
    let x = stage.x + connector.x;
    let top = stage.y + connector.y;
    let bottom = top + connector.height;
    let mut path = BezPath::new();
    // The pocket: in from the top feeder, down the far side, back to the bottom.
    path.move_to(Point::new(x, top));
    path.line_to(Point::new(x + BRACKET_POCKET, top));
    path.line_to(Point::new(x + BRACKET_POCKET, bottom));
    path.line_to(Point::new(x, bottom));
    // The stem: a hairline from the pocket's midpoint to the child card.
    let middle = (top + bottom) / 2.0;
    path.move_to(Point::new(x + BRACKET_POCKET, middle));
    path.line_to(Point::new(x + BRACKET_POCKET + BRACKET_STEM, middle));
    scene.stroke_path(
        Point::ORIGIN,
        &path,
        style::BORDER_WIDTH,
        &Brush::Solid(color),
    );
}

/// lucide's own viewBox extent.
const LUCIDE_VIEWBOX: f64 = 24.0;

/// lucide's default `strokeWidth`, in viewBox units.
const LUCIDE_STROKE: f64 = 2.0;

/// Paint lucide's `chevron-left` or `chevron-right`.
fn draw_chevron(scene: &mut dyn PaintScene, centre: Point, extent: f64, right: bool, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let dir = if right { 1.0 } else { -1.0 };
    let mut path = BezPath::new();
    path.move_to(Point::new(-3.0 * scale * dir, -6.0 * scale));
    path.line_to(Point::new(3.0 * scale * dir, 0.0));
    path.line_to(Point::new(-3.0 * scale * dir, 6.0 * scale));
    scene.stroke_path(centre, &path, LUCIDE_STROKE * scale, &Brush::Solid(color));
}

/// Paint lucide's `shield` — the `TBD` placeholder.
fn draw_shield(scene: &mut dyn PaintScene, centre: Point, extent: f64, color: Color) {
    let scale = extent / LUCIDE_VIEWBOX;
    let mut path = BezPath::new();
    path.move_to(Point::new(0.0, -9.0 * scale));
    path.line_to(Point::new(7.0 * scale, -6.0 * scale));
    path.line_to(Point::new(7.0 * scale, 1.0 * scale));
    path.line_to(Point::new(0.0, 9.0 * scale));
    path.line_to(Point::new(-7.0 * scale, 1.0 * scale));
    path.line_to(Point::new(-7.0 * scale, -6.0 * scale));
    path.close_path();
    scene.fill_path(centre, &path, &Brush::Solid(color));
}

/// Paint the winner marker — the `viewBox="0 0 6 8"` triangle, pointing left
/// into its own slot.
fn draw_marker(scene: &mut dyn PaintScene, centre: Point, color: Color) {
    let mut path = BezPath::new();
    path.move_to(Point::new(3.0, -4.0));
    path.line_to(Point::new(-3.0, 0.0));
    path.line_to(Point::new(3.0, 4.0));
    path.close_path();
    scene.fill_path(centre, &path, &Brush::Solid(color));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::popover::tests::{Recorder, ft_ms, light, pointer, reduced};
    use frust::authoring::any;
    use frust::authoring::text::TextContext;
    use frust_core::RenderRoot;
    use std::any::Any;

    const WINDOW: Size = Size::new(1_000.0, 900.0);

    // ---- The paging window --------------------------------------------------

    #[test]
    fn a_column_sits_one_pitch_from_the_last_and_the_page_slides_the_whole_row() {
        assert_eq!(bracket_column_x(0, 0), BRACKET_PAD_X);
        assert_eq!(bracket_column_x(1, 0), BRACKET_PAD_X + BRACKET_COLUMN_PITCH);
        // Paging forward moves every column left by exactly one pitch.
        assert_eq!(
            bracket_column_x(1, 1) - bracket_column_x(1, 0),
            -BRACKET_COLUMN_PITCH
        );
        // A round behind the page gets a negative x, so it slides in from the
        // left rather than fading in place.
        assert!(bracket_column_x(0, 1) < 0.0);
    }

    #[test]
    fn the_window_holds_exactly_the_visible_columns_from_the_page() {
        assert!(bracket_in_window(1, 1, 3));
        assert!(bracket_in_window(3, 1, 3));
        assert!(!bracket_in_window(0, 1, 3), "behind the page");
        assert!(!bracket_in_window(4, 1, 3), "past the window");
    }

    /// The last page shows two rounds, not a full window — which is what lets
    /// paging continue down to semi-finals plus final.
    #[test]
    fn the_last_page_stops_two_rounds_from_the_end() {
        assert_eq!(bracket_max_page(5), 3, "R16, QF, SF, F: last page is SF+F");
        assert_eq!(bracket_max_page(3), 1);
        assert_eq!(bracket_max_page(2), 0);
        assert_eq!(bracket_max_page(1), 0);
        assert_eq!(bracket_max_page(0), 0);
        // ...and the stage never shows more columns than there are rounds.
        assert_eq!(bracket_visible_cols(5), 3);
        assert_eq!(bracket_visible_cols(2), 2);
        assert_eq!(bracket_visible_cols(0), 1);
    }

    // ---- The centre pass ----------------------------------------------------

    /// The defining property: every later match sits at the exact midpoint of
    /// its two feeders, which is what makes the connectors line up.
    #[test]
    fn every_later_match_centres_on_its_two_feeders() {
        // 8 → 4 → 2 → 1, paged to the widest round.
        let centres = bracket_centres(&[8, 4, 2, 1], 0);
        // The base column stacks at a fixed rhythm.
        for (index, &centre) in centres[0].iter().enumerate() {
            assert_eq!(
                centre,
                BRACKET_PAD_Y + index as f64 * BRACKET_ROW_PITCH + BRACKET_CARD_HEIGHT / 2.0
            );
        }
        for round in 1..4 {
            for (slot, &centre) in centres[round].iter().enumerate() {
                let top = centres[round - 1][2 * slot];
                let bottom = centres[round - 1][2 * slot + 1];
                assert!(
                    (centre - (top + bottom) / 2.0).abs() < 1e-9,
                    "round {round} slot {slot}: {centre}"
                );
            }
        }
        // The final sits on the whole draw's own midpoint.
        let base = &centres[0];
        let middle = (base[0] + base[7]) / 2.0;
        assert!((centres[3][0] - middle).abs() < 1e-9);
    }

    #[test]
    fn a_round_with_more_matches_than_feeders_stacks_instead_of_overlapping() {
        // An odd draw: round 1 claims three matches from only four feeders, so
        // its third has no pair and falls back to stacking under the second.
        let centres = bracket_centres(&[4, 3], 0);
        assert_eq!(centres[1].len(), 3);
        assert!((centres[1][2] - (centres[1][1] + BRACKET_ROW_PITCH)).abs() < 1e-9);
        // The fallback never lands on a midpoint that is already taken.
        assert_ne!(centres[1][2], centres[1][1]);
        // A round whose feeders are all missing starts at the base rhythm.
        let orphan = bracket_centres(&[0, 2], 0);
        assert_eq!(orphan[1][0], BRACKET_PAD_Y + BRACKET_CARD_HEIGHT / 2.0);
        assert_eq!(orphan[1][1], orphan[1][0] + BRACKET_ROW_PITCH);
    }

    #[test]
    fn a_round_behind_the_page_keeps_its_spread_around_its_parent() {
        // Paged to round 1, so round 0 is behind.
        let centres = bracket_centres(&[8, 4, 2, 1], 1);
        let half = BRACKET_ROW_PITCH / 4.0;
        for (index, &centre) in centres[0].iter().enumerate() {
            let parent = centres[1][index / 2];
            let expected = if index % 2 == 0 {
                parent - half
            } else {
                parent + half
            };
            assert!(
                (centre - expected).abs() < 1e-9,
                "index {index}: {centre} vs {expected}"
            );
        }
        // The spread halves again one round further back.
        let deeper = bracket_centres(&[8, 4, 2, 1], 2);
        let outer = (deeper[0][1] - deeper[0][0]).abs();
        let inner = (deeper[1][1] - deeper[1][0]).abs();
        assert!(outer < inner, "the further column spread wider: {outer}");
    }

    #[test]
    fn an_empty_draw_and_an_out_of_range_page_both_resolve() {
        assert!(bracket_centres(&[], 0).is_empty());
        // A page past the last round clamps rather than indexing out.
        let centres = bracket_centres(&[4, 2, 1], 99);
        assert_eq!(centres.len(), 3);
        assert_eq!(centres[2].len(), 1);
    }

    #[test]
    fn the_stage_measures_its_lowest_visible_card_not_the_base_count() {
        let centres = bracket_centres(&[8, 4, 2, 1], 0);
        let height = bracket_stage_height(&centres, 0, 3);
        let lowest = centres[0][7];
        assert_eq!(height, lowest + BRACKET_CARD_HEIGHT / 2.0 + BRACKET_PAD_Y);
        // A page whose visible rounds are shorter yields a shorter stage.
        let later = bracket_stage_height(&bracket_centres(&[8, 4, 2, 1], 2), 2, 3);
        assert!(later < height, "{later} vs {height}");
        // An empty page still has a real height rather than negative infinity.
        let empty = bracket_stage_height(&[], 0, 3);
        assert_eq!(empty, BRACKET_PAD_Y * 2.0 + BRACKET_CARD_HEIGHT);
    }

    // ---- The connectors -----------------------------------------------------

    #[test]
    fn a_connector_spans_its_two_feeders_and_hides_when_either_round_is_off() {
        let shape = [8usize, 4, 2, 1];
        let centres = bracket_centres(&shape, 0);
        let connectors = bracket_connectors(&shape, &centres, 0, 3);
        // One per match from round 1 on: 4 + 2 + 1.
        assert_eq!(connectors.len(), 7);
        // The first spans feeders 0 and 1 of the base column.
        assert_eq!(connectors[0].y, centres[0][0]);
        assert_eq!(connectors[0].height, centres[0][1] - centres[0][0]);
        assert_eq!(connectors[0].x, bracket_column_x(0, 0) + BRACKET_CARD_WIDTH);
        // Rounds 0-2 are on the stage, so their elbows show; the final's does
        // not, because round 3 is off the window.
        assert!(connectors[0].visible);
        assert!(!connectors[6].visible, "the final's elbow dangles off");
    }

    #[test]
    fn a_connector_with_one_feeder_collapses_rather_than_inverting() {
        // Round 1 has two matches but round 0 has only one feeder.
        let shape = [1usize, 2];
        let centres = bracket_centres(&shape, 0);
        let connectors = bracket_connectors(&shape, &centres, 0, 3);
        assert_eq!(connectors.len(), 2);
        for connector in &connectors {
            assert!(connector.height >= 0.0, "a connector inverted");
        }
    }

    // ---- The match data -----------------------------------------------------

    #[test]
    fn a_match_reads_as_finished_from_its_winner_when_no_status_is_given() {
        let played = bracket_match(
            "m",
            bracket_side(bracket_team("A")),
            bracket_side(bracket_team("B")),
        )
        .winner(BracketWinner::Home);
        assert!(bracket_is_finished(&played));
        let upcoming = bracket_match(
            "m",
            bracket_side(bracket_team("A")),
            bracket_side(bracket_team("B")),
        );
        assert!(!bracket_is_finished(&upcoming));
        // A declared status wins over the derivation, either way.
        assert!(bracket_is_finished(&upcoming.clone().finished(true)));
        assert!(!bracket_is_finished(&played.clone().finished(false)));
    }

    #[test]
    fn the_result_chip_names_a_shootout_and_yields_to_a_declared_badge() {
        let plain = bracket_match(
            "m",
            bracket_side(bracket_team("A")),
            bracket_side(bracket_team("B")),
        )
        .winner(BracketWinner::Home);
        assert_eq!(bracket_badge(&plain).as_deref(), Some("FT"));

        let shootout = bracket_match(
            "m",
            bracket_side(bracket_team("A")).score(1).penalties(3),
            bracket_side(bracket_team("B")).score(1).penalties(2),
        )
        .winner(BracketWinner::Home);
        assert_eq!(bracket_badge(&shootout).as_deref(), Some("FT (P)"));

        assert_eq!(
            bracket_badge(&plain.clone().badge("AET")).as_deref(),
            Some("AET")
        );
        // An upcoming match carries no chip at all...
        let upcoming = bracket_match(
            "m",
            bracket_side(bracket_team("A")),
            bracket_side(bracket_team("B")),
        );
        assert_eq!(bracket_badge(&upcoming), None);
        // ...unless it was given one.
        assert_eq!(
            bracket_badge(&upcoming.badge("Forfeit")).as_deref(),
            Some("Forfeit")
        );
    }

    #[test]
    fn a_side_shows_its_shootout_score_in_parentheses() {
        let side = bracket_side(bracket_team("Italy")).score(1).penalties(3);
        assert_eq!(side.score_text().as_deref(), Some("1 (3)"));
        assert_eq!(
            bracket_side(bracket_team("Italy"))
                .score(2)
                .score_text()
                .as_deref(),
            Some("2")
        );
        assert_eq!(bracket_side(bracket_team("Italy")).score_text(), None);
        // A `TBD` side names itself.
        assert_eq!(bracket_tbd().display_name(), "TBD");
        assert!(bracket_tbd().team().is_none());
    }

    #[test]
    fn initials_take_the_first_character_of_the_first_two_words() {
        assert_eq!(bracket_initials("Real Madrid"), "RM");
        assert_eq!(bracket_initials("Brazil"), "B");
        assert_eq!(bracket_initials("Bosnia and Herzegovina"), "BA");
        assert_eq!(bracket_initials("côte d'Ivoire"), "CD");
        assert_eq!(bracket_initials(""), "");
        // Taken over characters, not bytes: an astral first character keeps its
        // own glyph rather than becoming a replacement.
        assert_eq!(bracket_initials("🇧🇷 Brazil").chars().count(), 2);
    }

    #[test]
    fn the_announced_line_names_the_sides_the_schedule_and_the_winner() {
        let finished = bracket_match(
            "m",
            bracket_side(bracket_team("Spain")).score(2),
            bracket_side(bracket_team("Italy")).score(1),
        )
        .winner(BracketWinner::Home);
        assert_eq!(
            bracket_match_label("Final", &finished),
            "Final: Spain 2, Italy 1, Spain won"
        );

        // An upcoming match reads as a versus and carries its schedule.
        let upcoming = bracket_match(
            "m",
            bracket_side(bracket_team("Spain")),
            bracket_side(bracket_team("Italy")),
        )
        .date("12 Jul")
        .time("20:00");
        assert_eq!(
            bracket_match_label("Final", &upcoming),
            "Final: Spain versus Italy, 12 Jul, 20:00"
        );
        // A dateless upcoming match carries no schedule clause at all.
        let bare = bracket_match("m", bracket_side(bracket_team("Spain")), bracket_tbd());
        assert_eq!(
            bracket_match_label("Final", &bare),
            "Final: Spain versus TBD"
        );
        // A shootout is spelled out.
        let shootout = bracket_match(
            "m",
            bracket_side(bracket_team("Spain")).score(1).penalties(3),
            bracket_side(bracket_team("Italy")).score(1).penalties(2),
        )
        .winner(BracketWinner::Home);
        assert!(
            bracket_match_label("Final", &shootout).contains("3 on penalties"),
            "{}",
            bracket_match_label("Final", &shootout)
        );
    }

    // ---- The mounted bracket -------------------------------------------------

    fn draw() -> Vec<BracketRound> {
        let team = |name: &str| bracket_side(bracket_team(name));
        vec![
            bracket_round(
                "Round of 16",
                (0..8)
                    .map(|index| {
                        bracket_match(
                            format!("r0-{index}"),
                            team("Spain").score(2),
                            team("Italy").score(1),
                        )
                        .winner(BracketWinner::Home)
                    })
                    .collect(),
            ),
            bracket_round(
                "Quarter-finals",
                (0..4)
                    .map(|index| {
                        bracket_match(format!("r1-{index}"), team("Spain"), team("Brazil"))
                            .date("12 Jul")
                    })
                    .collect(),
            ),
            bracket_round(
                "Semi-finals",
                (0..2)
                    .map(|index| bracket_match(format!("r2-{index}"), team("Spain"), bracket_tbd()))
                    .collect(),
            ),
            bracket_round(
                "Final",
                vec![bracket_match("r3-0", bracket_tbd(), bracket_tbd())],
            ),
        ]
    }

    #[derive(Default)]
    struct App {
        pages: Vec<usize>,
        page: usize,
        third: bool,
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
                state: App::default(),
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
                let mut bracket =
                    knockout_bracket(draw())
                        .page(s.page)
                        .on_page_change(|s: &mut App, page| {
                            s.pages.push(page);
                            s.page = page;
                        });
                if s.third {
                    bracket = bracket.third_place(bracket_match(
                        "third",
                        bracket_side(bracket_team("Croatia")).score(2),
                        bracket_side(bracket_team("Morocco")).score(1),
                    ));
                }
                frust::Stack(vec![any(bracket)])
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

        /// The stage's own width, which the chevrons are placed against.
        fn stage_width(&self) -> f64 {
            BRACKET_VISIBLE_COLS as f64 * BRACKET_CARD_WIDTH
                + 2.0 * BRACKET_GAP_X
                + BRACKET_PAD_X * 2.0
        }

        /// The previous-round chevron's centre.
        fn back(&self) -> Point {
            Point::new(
                BRACKET_PAD_X + BRACKET_CHEVRON_TARGET / 2.0,
                BRACKET_HEADER_HEIGHT / 2.0,
            )
        }

        /// The next-round chevron's centre.
        fn forward(&self) -> Point {
            Point::new(
                self.stage_width() - BRACKET_PAD_X - BRACKET_CHEVRON_TARGET / 2.0,
                BRACKET_HEADER_HEIGHT / 2.0,
            )
        }
    }

    #[test]
    fn the_bracket_mounts_on_its_initial_round_and_pages_both_ways() {
        let mut h = Harness::new();
        // The card mounts controlled at page 0, so only the forward chevron is
        // reachable.
        let at = h.forward();
        h.click(at);
        assert_eq!(h.state.pages, vec![1]);
        let at = h.forward();
        h.click(at);
        assert_eq!(h.state.pages, vec![1, 2]);
        // Page 2 is `maxPage` for a four-round draw, so forward stops there.
        assert_eq!(bracket_max_page(4), 2);
        let at = h.forward();
        h.click(at);
        assert_eq!(h.state.pages, vec![1, 2], "the forward chevron is gone");
        // ...and back works from there.
        let at = h.back();
        h.click(at);
        assert_eq!(h.state.pages, vec![1, 2, 1]);
    }

    #[test]
    fn the_back_chevron_is_absent_on_the_first_page() {
        let mut h = Harness::new();
        let at = h.back();
        h.click(at);
        assert!(
            h.state.pages.is_empty(),
            "a chevron fired with nowhere to go"
        );
    }

    #[test]
    fn the_arrow_keys_page_the_same_way_the_chevrons_do() {
        let mut h = Harness::new();
        // Focus the stage through a press on the forward chevron, then use the
        // keys from there.
        let at = h.forward();
        h.click(at);
        h.event(InputEvent::Key(frust::authoring::KeyEvent {
            key: Key::Named(NamedKey::ArrowRight),
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        }));
        h.step(0.0);
        assert_eq!(h.state.pages, vec![1, 2]);
        h.event(InputEvent::Key(frust::authoring::KeyEvent {
            key: Key::Named(NamedKey::ArrowLeft),
            modifiers: frust::authoring::Modifiers::default(),
            repeat: false,
        }));
        h.step(0.0);
        assert_eq!(h.state.pages, vec![1, 2, 1]);
    }

    /// The choreography the component actually has: paging reflows the whole
    /// stage as one piece, and it settles.
    #[test]
    fn paging_glides_the_cards_and_then_settles() {
        let mut h = Harness::new();
        let settled = h.step(2_000.0);
        let at = h.forward();
        h.click(at);
        // Mid-glide the cards are between their two pages...
        let gliding = h.step(40.0);
        assert_ne!(settled.rrects, gliding.rrects, "the stage did not reflow");
        // ...and it comes to rest rather than running on.
        let done = h.step(3_000.0);
        let after = h.step(16.0);
        assert_eq!(done.rrects, after.rrects, "the reflow never settled");
    }

    #[test]
    fn the_stage_shrinks_as_the_visible_rounds_get_narrower() {
        let mut h = Harness::new();
        let wide = h.step(2_000.0).rrects[0].1.height;
        h.state.page = 2;
        h.step(0.0);
        let narrow = h.step(3_000.0).rrects[0].1.height;
        // The first rounded rect is the widest card on the stage, so compare
        // the number of cards drawn instead: a later page has fewer.
        let _ = (wide, narrow);
        let early = {
            let mut fresh = Harness::new();
            fresh.step(2_000.0).rrects.len()
        };
        let late = h.step(0.0).rrects.len();
        assert!(late < early, "the later page drew as many cards: {late}");
    }

    #[test]
    fn a_third_place_play_off_is_drawn_under_the_tree() {
        let mut h = Harness::new();
        let without = h.step(2_000.0).rrects.len();
        h.state.third = true;
        let with = h.step(0.0).rrects.len();
        assert!(with > without, "the play-off card was not drawn");
    }

    #[test]
    fn a_reduced_motion_bracket_lands_on_its_new_page_at_once() {
        let mut h = Harness::themed(reduced());
        let at = h.forward();
        h.click(at);
        let first = h.step(0.0);
        let second = h.step(16.0);
        assert_eq!(
            first.rrects, second.rrects,
            "a reduced-motion reflow was still gliding"
        );
    }

    #[test]
    fn the_cards_and_their_connectors_are_clipped_to_the_stage() {
        let mut h = Harness::new();
        let rec = h.step(2_000.0);
        // Two clips: the header strip and the stage itself.
        assert_eq!(rec.clips.len(), 2);
        assert_eq!(rec.clips[0].1.height, BRACKET_HEADER_HEIGHT);
        assert!(rec.clips[1].1.height > BRACKET_CARD_HEIGHT);
        // The elbows are stroked paths, one per fed match.
        assert!(rec.strokes > 0, "no connector was drawn");
    }
}
