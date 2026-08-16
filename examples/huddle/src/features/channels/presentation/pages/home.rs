//! Home tab — channels + DMs.
//!
//! A [`Component`] hosting a [`ChannelsController`](crate::features::channels)
//! (via `clean_signals_frust::use_controller`, the same seam
//! `settings_appearance` uses) that loads a channel + DM roster with a
//! deliberate mock latency so the loading **skeletons** are visible. Each row
//! is a [`swipeable_row`](crate::ui::swipeable::swipeable_row): swipe right to
//! archive, left to mute, each raising an undo toast. A status-dot avatar (two
//! rendered with [`Image`], the rest an initials circle painted via
//! `frust::authoring`), an unread badge, and a lock icon for private
//! channels complete each row. Pull-to-refresh re-runs the loader.
//!
//! # Why the hand-rolled widgets
//!
//! No facade widget paints an arbitrary-color filled circle or an animated
//! shimmer, so [`fill_box`](crate::ui::fill_box) (promoted to `crate::ui` so
//! the feed can share it) and [`Shimmer`] below
//! are small hand-rolled `View`/`Widget` pairs built directly against
//! `frust::authoring` — the same
//! precedent the pre-skeleton theme screen used for its `ColorBoxView`.
//! Everything else goes through the `frust` facade.
//!
//! # Roster container: a keyed, variable-extent `ListView`
//!
//! The roster renders through [`ListView::builder_keyed`](frust::ListView::builder_keyed)
//! over a flat row list — the two singleton section header rows
//! ("Channels" and "Direct messages") bracketing [`RosterRow::Channel`] and
//! [`RosterRow::Dm`] rows, rebuilt fresh from
//! [`ChannelsController::data`](crate::features::channels::ChannelsController::data)
//! every frame exactly like the earlier `keyed` `Column`'s children were —
//! keyed by [`RosterRow::key`] (channel or DM id for real items, distinct
//! collision-proof string keys for the two singletons). A channel/DM row's
//! avatar-driven height and a section header's shorter text-only height are
//! *not* the same, so the list runs in **variable-extent** mode
//! ([`ListView::estimated_item_extent`](frust::ListView::estimated_item_extent),
//! seeded with [`ROSTER_ROW_EXTENT`] — the feed's precedent, `channel_feed`)
//! rather than the closed-form uniform path: each row measures its own
//! natural height (a header stays short; a channel/DM row's avatar +
//! `row_layout` padding still comes out to exactly `ROSTER_ROW_EXTENT`) —
//! forcing a header to the taller row extent (an earlier, reverted attempt at
//! this) shifted every following row down far enough that a channel appended
//! at the roster's live edge could land outside a caller's viewport.
//! [`on_refresh_release`](frust::ListView::on_refresh_release) drives the
//! pull-to-refresh operation at the same refresh op the earlier `scroll_view`
//! version used — [`roster_list`] overlays its own centered spinner while a
//! reload is in flight, since the callback is a pure gesture hook with no
//! in-progress visual of its own. Keying by id means a mute/archive/reorder
//! mutation keeps swipe/press state attached to the right channel — the
//! keyed identity that the old positional Column couldn't maintain.
//!
//! # Home actions
//!
//! The app bar's leading "HQ" tile is the workspace-switcher's first real
//! entry point (the `/workspace-switcher` route already existed but nothing
//! pushed it — see `screens::workspace_drawer`'s module docs); the trailing
//! `icons::ADD` action opens a create-channel bottom sheet (reusing
//! [`crate::ui::sheet`]'s existing sheet primitive) that appends straight to
//! [`ChannelsController`]'s live list via
//! [`ChannelsController::create_channel`]. Long-pressing a row (channel or
//! DM — [`GestureDetector::on_long_press`]) opens a Mute/Unmute · Archive ·
//! Invite people · Cancel action-sheet menu; Mute/Archive reuse the exact
//! controller ops + undo toast [`swipe_wrap`] already wires (swipe parity).
//! **"Invite people" is the one chosen entry point into the invite
//! modal** (a row on the create-channel sheet's success toast was also
//! considered, but a toast action is a `ToastController` affordance meant
//! for *undo*, not for opening a second modal on top of a just-dismissed
//! one, so the long-press menu is the cleaner single seam): it pushes a
//! `dialog`/`cupertino_alert` (picked by the live [`DesignLanguage`])
//! confirm/cancel modal onto the app's outer navigator, and confirming
//! toasts "Invites sent (mock)" — see [`show_invite_modal`].

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use frust::authoring::{
    BoxConstraints, BuildCtx, ChangeFlags, Color, LayoutCtx, PaintCtx, PaintScene, Size, Widget,
};
use frust::{
    Align, Alignment, AnimationController, AnyView, Axis, ChildKey, Column, CrossAxisAlignment,
    DesignLanguage, EdgeInsets, FlexView, GestureDetector, Get, Image, ImageFit, ImageSource,
    ListView, NavigatorController, Padding, PopResult, ProgressValue, Row, SizedBox, Stack, Theme,
    View, action, any, app_bar, button, circular_progress, component, dialog, flexible, hero, icon,
    icons, inflexible, safe_area, scroll_view, show_cupertino_alert, show_dialog, switch, text,
    text_input, use_context,
};

use clean_signals_frust::use_controller;

use crate::HuddleState;
use crate::failure::HuddleFailure;
use crate::features::channels::domain::repositories::ChannelRepository;
use crate::features::channels::{self, ChannelItem, ChannelsController, DmItem};
use crate::features::profile::domain::UserStatus;
use crate::ui::fill_box::fill_box;
use crate::ui::sheet::{action_menu, sheet, sheet_action_row};

// ---------------------------------------------------------------------------
// Palette / tunables
// ---------------------------------------------------------------------------

/// Swipe-right (archive) accent — a confirming green.
const ARCHIVE_COLOR: Color = Color::from_rgb8(0x2E, 0x7D, 0x32);
/// Swipe-left (mute) accent — a neutral slate.
const MUTE_COLOR: Color = Color::from_rgb8(0x54, 0x6E, 0x7A);
/// Unread badge fill.
const BADGE_COLOR: Color = Color::from_rgb8(0xD3, 0x2F, 0x2F);
/// The `#`-circle tint for channel rows.
const CHANNEL_TINT: Color = Color::from_rgb8(0x5B, 0x53, 0x7D);

/// The initials-avatar background palette, indexed by user id.
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

/// The status-dot color for a presence state.
fn status_color(status: UserStatus) -> Color {
    match status {
        UserStatus::Online => Color::from_rgb8(0x43, 0xA0, 0x47),
        UserStatus::Away => Color::from_rgb8(0xFB, 0x8C, 0x00),
        UserStatus::Dnd => Color::from_rgb8(0xE5, 0x39, 0x35),
    }
}

/// The roster [`ListView`](frust::ListView)'s variable-extent estimate, and a
/// channel/DM row's actual measured height: avatar column (40) + row padding
/// (20, split across top and bottom via [`row_layout`]'s
/// `EdgeInsets::symmetric`), for a total of 60px. A section header measures
/// shorter than this (its own natural text height) — see the [module
/// docs](self)' *Roster container* section for why the list is
/// variable-extent rather than closed-form uniform.
const ROSTER_ROW_EXTENT: f64 = 60.0;

// ---------------------------------------------------------------------------
// Material list metrics (Material sizing reference measurements)
// ---------------------------------------------------------------------------

/// CircleAvatar diameter — radius 20 → 40 (`circle_avatar.dart:136-138`).
const AVATAR_SIZE: f64 = 40.0;
/// Avatar corner radius: a full circle at [`AVATAR_SIZE`].
const AVATAR_RADIUS: f64 = AVATAR_SIZE / 2.0;
/// The `#`/initials monogram inside an avatar circle (a bodyLarge-ish glyph).
const MONOGRAM_SIZE: f32 = 16.0;
/// List title role — M3 `titleMedium` (w500), 16 (`typography.dart:2097-2111`).
const LIST_TITLE_SIZE: f32 = 16.0;
/// List subtitle / preview role — M3 `bodyMedium`, 14 (same table).
const LIST_SUBTITLE_SIZE: f32 = 14.0;
/// Section-header label — M3 `labelLarge`-ish (13).
const SECTION_LABEL_SIZE: f32 = 13.0;
/// Unread-badge pill — a compact 18px-tall counter (Material badge convention).
const BADGE_W: f64 = 22.0;
const BADGE_H: f64 = 18.0;
const BADGE_RADIUS: f64 = BADGE_H / 2.0;
/// Unread-badge count text — M3 `labelSmall` (11).
const BADGE_TEXT_SIZE: f32 = 11.0;

// ---------------------------------------------------------------------------
// Roster row structure
// ---------------------------------------------------------------------------

/// One row of the virtualized roster list — the two ephemeral singleton rows
/// bracketing a real channel or DM, exactly the shape the earlier `keyed`
/// `Column` built by hand each frame. See the [module docs](self)' *Roster
/// container* section.
enum RosterRow {
    /// The "Channels" header row, shown at the top of the roster.
    ChannelsHeader,
    /// A real channel item.
    Channel(ChannelItem),
    /// The "Direct messages" header row.
    DmsHeader,
    /// A real DM item.
    Dm(DmItem),
}

impl RosterRow {
    /// This row's stable [`ChildKey`] — a channel/DM's id for real items,
    /// or a collision-proof string key for the two singleton headers (distinct
    /// from any channel/DM id: [`ChildKey::new`] hashes the value *and* its
    /// type, and an id is a `String`, never matching the `&str` header keys).
    fn key(&self) -> ChildKey {
        match self {
            RosterRow::ChannelsHeader => ChildKey::new("channels_header"),
            RosterRow::Channel(c) => ChildKey::new(&c.id),
            RosterRow::DmsHeader => ChildKey::new("dms_header"),
            RosterRow::Dm(d) => ChildKey::new(&d.id),
        }
    }
}

// ---------------------------------------------------------------------------
// Escape-hatch leaf widgets
// ---------------------------------------------------------------------------

// `FillBox`/`fill_box` were promoted to `crate::ui::fill_box`
// so the feed restyle can share the avatar/
// status-dot/badge/composer-tile primitive; imported at the top of this module.

/// A skeleton shimmer bar: a gray rounded rect whose alpha pulses via an
/// [`AnimationController`] (the framework's paint-driven animation contract).
struct Shimmer {
    size: Size,
    radius: f64,
}

/// The retained widget for a [`Shimmer`].
struct ShimmerWidget {
    size: Size,
    radius: f64,
    anim: AnimationController,
}

/// Construct a [`Shimmer`] bar.
fn shimmer(size: Size, radius: f64) -> Shimmer {
    Shimmer { size, radius }
}

impl<State: 'static> View<State> for Shimmer {
    type Element = ShimmerWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> ShimmerWidget {
        let mut anim = AnimationController::new(Duration::from_millis(1100));
        anim.repeat();
        ShimmerWidget {
            size: self.size,
            radius: self.radius,
            anim,
        }
    }

    fn rebuild(
        &self,
        _prev: &Self,
        element: &mut ShimmerWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.size = self.size;
        element.radius = self.radius;
        ChangeFlags::PAINT
    }
}

impl Widget for ShimmerWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(self.size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        let animating = self.anim.advance(ctx.frame_time());
        // Triangle wave 0→1→0 over the loop, mapped to a gentle alpha pulse.
        let v = self.anim.value();
        let tri = 1.0 - (2.0 * v - 1.0).abs();
        let alpha = (0.18 + 0.20 * tri).clamp(0.0, 1.0);
        let color = Color::from_rgba8(0xB0, 0xB0, 0xB0, (alpha * 255.0) as u8);
        scene.fill_rounded_rect(ctx.origin(), ctx.size(), self.radius, color);
        if animating {
            ctx.request_frame();
        }
    }
}

// ---------------------------------------------------------------------------
// Screen component
// ---------------------------------------------------------------------------

/// The Home tab root: a `Component` hosting the roster controller. `nav` lets
/// row/avatar taps push detail pages.
pub fn home_screen(nav: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    any(component(HomeScreen { nav }))
}

/// The Home screen `Component` (stateless config; state lives in [`HomeState`]).
struct HomeScreen {
    nav: NavigatorController<HuddleState>,
}

/// Retained Home state.
struct HomeState {
    controller: Arc<ChannelsController>,
    nav: NavigatorController<HuddleState>,
    toasts: crate::ui::toast::ToastController,
    /// A decoded image used for a couple of DM avatars (`None` if decode fails).
    logo: Option<ImageSource>,
    /// Which Home overlay sheet (if any) is open — plain retained
    /// `Component` state, not an `RwSignal`: a tap/long-press callback mutates
    /// it directly via `EventCtx::state_mut::<HomeState>()`, and the change is
    /// picked up on the very next rebuild like any other retained field (see
    /// `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions).
    sheet: HomeSheet,
    /// The create-channel sheet's draft name field.
    create_name: String,
    /// The create-channel sheet's draft private toggle.
    create_private: bool,
}

/// Which Home-screen overlay sheet (if any) is open — see the [module
/// docs](self)' "Home actions" section.
#[derive(Clone)]
enum HomeSheet {
    /// No sheet — the roster is fully interactive.
    None,
    /// The app-bar ADD action's create-channel sheet.
    Create,
    /// The long-press action menu for row `id` (`label` for the toast text,
    /// `muted` for the Mute/Unmute row's current-state label).
    RowActions {
        id: String,
        label: String,
        muted: bool,
    },
}

impl frust::Component for HomeScreen {
    type State = HomeState;

    fn init(&self) -> HomeState {
        let toasts = use_context::<crate::ui::toast::ToastController>().unwrap_or_default();
        // Recover the repository the composition root published under the root
        // Owner (see `crate::HuddleApp::init`) — the injection seam that reaches
        // both this page's route-table construction and its in-screen
        // `nav.push` re-construction.
        let repo = use_context::<Arc<dyn ChannelRepository + Send + Sync>>()
            .expect("the composition root provides a ChannelRepository");
        let controller = use_controller::<ChannelsController, HuddleFailure>(move || {
            ChannelsController::new(repo)
        });

        // Kick off the initial load on the UI-thread task queue (the canonical
        // clean-signals-frust pattern — `use_interval` reloads the same way;
        // the ~600ms timer resolves via the reactive runtime's tokio context,
        // which `pump_local` enters). The latency makes the skeletons visible.
        let handle = controller.clone();
        frust::spawn_local(async move {
            handle.load().await;
        });

        let logo = ImageSource::decode(include_bytes!("../../../../../assets/logo.png")).ok();

        HomeState {
            controller,
            nav: self.nav.clone(),
            toasts,
            logo,
            sheet: HomeSheet::None,
            create_name: String::new(),
            create_private: false,
        }
    }

    fn build(&self, state: &mut HomeState) -> AnyView<HomeState> {
        let async_state = state.controller.data.get(); // tracked

        let body: AnyView<HomeState> = if let Some(data) = async_state.value() {
            roster_list(state, data, async_state.is_loading())
        } else if async_state.is_loading() {
            skeleton_list()
        } else {
            any(Align(
                Alignment::CENTER,
                text("Couldn't load channels.").size(15.0),
            ))
        };

        let design = use_context::<Theme>()
            .map(|t| t.design_language)
            .unwrap_or(DesignLanguage::Material3);

        // The app bar clears the top status-bar/cutout inset — its own
        // background still only fills the un-padded bar height (see
        // `shell::bottom_bar`'s doc for the same known v1 gap on the
        // opposite edge).
        let bar = any(safe_area(
            app_bar::<HomeState>("Huddle")
                .leading(workspace_tile(&state.nav))
                .actions(vec![create_channel_action()]),
        )
        .bottom(false));

        let screen = any(
            FlexView::new(Axis::Vertical, vec![inflexible(bar), flexible(1, body)])
                .cross_axis(CrossAxisAlignment::Stretch),
        );

        // The open Home overlay sheet mounts in the screen's own `Stack` top
        // layer (mirrors `screens::channel_feed`'s `feed_sheet`); when
        // closed it is an inert zero-size box, so the roster stays interactive.
        let overlay = home_sheet_overlay(state, design);
        any(Stack(vec![screen, overlay]))
    }
}

// ---------------------------------------------------------------------------
// App-bar actions
// ---------------------------------------------------------------------------

/// The app bar's leading "HQ" initials tile — the same [`fill_box`] +
/// centered-label escape-hatch shape [`channel_circle`] already uses (rather
/// than `filled_card`, whose 16px content inset would balloon a 40px tile
/// past the 64dp bar's own height). Tapping it pushes `/workspace-switcher`
/// onto the outer app navigator (see the [module docs](self)' "Home actions"
/// section) — an entry point the drawer previously lacked.
fn workspace_tile(nav: &NavigatorController<HuddleState>) -> AnyView<HomeState> {
    let nav = nav.clone();
    let tile = Padding(
        EdgeInsets::symmetric(6.0, 12.0),
        // SizedBox+Align monogram idiom — see `channel_circle`.
        Stack(vec![
            any(fill_box(
                Size::new(AVATAR_SIZE, AVATAR_SIZE),
                CHANNEL_TINT,
                12.0,
            )),
            any(SizedBox(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(Align(
                Alignment::CENTER,
                text("HQ").size(12.0).color(Color::WHITE),
            ))),
        ]),
    );
    any(GestureDetector(tile).on_tap(move |_s: &mut HomeState| {
        nav.push(crate::features::channels::presentation::pages::workspace_drawer::workspace_drawer_screen);
    }))
}

/// The app bar's trailing `icons::ADD` action — opens the create-channel
/// sheet with a fresh (empty, public) draft.
fn create_channel_action() -> AnyView<HomeState> {
    any(
        GestureDetector(icon(icons::ADD).size(24.0).label("Create a channel")).on_tap(
            |s: &mut HomeState| {
                s.sheet = HomeSheet::Create;
                s.create_name.clear();
                s.create_private = false;
            },
        ),
    )
}

// ---------------------------------------------------------------------------
// Loading skeleton
// ---------------------------------------------------------------------------

/// The loading skeleton: shimmer rows standing in for the roster.
fn skeleton_list() -> AnyView<HomeState> {
    let mut rows: Vec<AnyView<HomeState>> = Vec::new();
    for _ in 0..7 {
        rows.push(skeleton_row());
    }
    any(scroll_view(Padding(EdgeInsets::all(8.0), Column(rows))))
}

/// One skeleton row: a shimmer circle + two shimmer lines.
fn skeleton_row() -> AnyView<HomeState> {
    any(Padding(
        EdgeInsets::symmetric(8.0, 10.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(any(shimmer(
                    Size::new(AVATAR_SIZE, AVATAR_SIZE),
                    AVATAR_RADIUS,
                ))),
                inflexible(any(SizedBox(Some(12.0), None))),
                flexible(
                    1,
                    any(Column(vec![
                        any(shimmer(Size::new(150.0, 14.0), 4.0)),
                        any(SizedBox(None, Some(6.0))),
                        any(shimmer(Size::new(220.0, 12.0), 4.0)),
                    ])),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ))
}

// ---------------------------------------------------------------------------
// Loaded roster
// ---------------------------------------------------------------------------

/// The loaded roster: the keyed, variable-extent `ListView` with section
/// headers and channel/DM rows, wrapped in a pull-to-refresh `Padding` layer,
/// with a centered indeterminate spinner overlaid while `refreshing` (a
/// reload triggered by pull-to-refresh) is in flight. See the [module
/// docs](self)' *Roster container* section.
fn roster_list(
    state: &HomeState,
    data: &channels::ChannelsData,
    refreshing: bool,
) -> AnyView<HomeState> {
    // Build the roster row list: "Channels" header, then channels, then
    // "Direct messages" header, then DMs. This mirrors the earlier keyed
    // Column structure built by hand each frame.
    let mut rows: Vec<RosterRow> = Vec::new();
    rows.push(RosterRow::ChannelsHeader);
    rows.extend(data.channels.iter().cloned().map(RosterRow::Channel));
    rows.push(RosterRow::DmsHeader);
    rows.extend(data.dms.iter().cloned().map(RosterRow::Dm));

    let row_count = rows.len();
    let rows = Rc::new(rows);

    // The key function: each row's stable identity, used to reconcile rows
    // across mutations like mute/archive/reorder.
    let key_rows = Rc::clone(&rows);
    let key_of = move |i: usize| key_rows[i].key();

    // Extract the pieces we need from state so they can be captured in 'static
    // closures: NavigatorController and ToastController are cloned/shared into
    // the builder, along with the Arc<ChannelsController> (cheap clone).
    let build_rows = Rc::clone(&rows);
    let build_nav = state.nav.clone();
    let build_controller = Arc::clone(&state.controller);
    let build_toasts = state.toasts.clone();
    let build_logo = state.logo.clone();

    // The builder function: construct the view for each row. Each row needs
    // the pieces for navigation, async ops, and UI state that were extracted
    // above.
    let builder = move |i: usize| match &build_rows[i] {
        RosterRow::ChannelsHeader => section_header("Channels"),
        RosterRow::Channel(c) => {
            channel_row_keyed(&build_nav, &build_controller, &build_toasts, &build_logo, c)
        }
        RosterRow::DmsHeader => section_header("Direct messages"),
        RosterRow::Dm(d) => {
            dm_row_keyed(&build_nav, &build_controller, &build_toasts, &build_logo, d)
        }
    };

    // The refresh callback: same operation the earlier scroll_view used.
    let pager = Arc::clone(&state.controller);
    let list = ListView::builder_keyed(row_count, ROSTER_ROW_EXTENT, key_of, builder)
        .estimated_item_extent(ROSTER_ROW_EXTENT)
        .on_refresh_release(move |_s: &mut HomeState| {
            let handle = pager.clone();
            frust::spawn_local(async move {
                handle.load().await;
            });
        });

    // The outer padding is the same 8px horizontal that the old scroll_view
    // used (and the vertical breathing room is unchanged too, just now applied
    // to the list's viewport rather than the column's content).
    let list = any(Padding(EdgeInsets::all(8.0), list));

    if !refreshing {
        return list;
    }

    // While a pull-to-refresh reload is in flight, overlay a centered
    // indeterminate spinner on top of the (still-visible, stale) roster —
    // `ListView::on_refresh_release`'s own rubber-band overscroll is a pure
    // gesture/position effect with no in-progress visual of its own, so the
    // app supplies one, exactly as the pre-`ListView` `scroll_view` version
    // did. A `Stack` layer rather than a synthetic row, so it never
    // participates in row keying/windowing.
    any(Stack(vec![
        list,
        any(Align(
            Alignment::new(0.0, -0.85), // near the top, matching the old top-of-content spinner
            Padding(
                EdgeInsets::all(8.0),
                circular_progress(ProgressValue::Indeterminate),
            ),
        )),
    ]))
}

/// A section header row: left-aligned text at SECTION_LABEL_SIZE, its own
/// natural height (shorter than a channel/DM row's avatar-driven
/// [`ROSTER_ROW_EXTENT`]) — safe under the roster's variable-extent `ListView`
/// (see [`roster_list`]), which measures each row rather than forcing every
/// row to one uniform height.
fn section_header(title: &str) -> AnyView<HomeState> {
    any(Padding(
        // Start margin 16 (8 outer list pad + 8 here) — Material side margin.
        EdgeInsets::symmetric(8.0, 12.0),
        text(title.to_string()).size(SECTION_LABEL_SIZE),
    ))
}

/// Wrap a row (leading + tappable content, pre-swipe) so a long-press opens
/// its [`HomeSheet::RowActions`] menu. [`GestureDetector`] is a transparent
/// wrapper (every event still forwards to the child — see its module docs),
/// so this composes cleanly with the row's own tap navigation and the
/// swipe wrapper that wraps it next.
fn row_with_long_press_menu(
    row: AnyView<HomeState>,
    id: String,
    label: String,
    muted: bool,
) -> AnyView<HomeState> {
    any(
        GestureDetector(row).on_long_press(move |s: &mut HomeState| {
            s.sheet = HomeSheet::RowActions {
                id: id.clone(),
                label: label.clone(),
                muted,
            };
        }),
    )
}

/// The leading `#` circle for a channel row.
///
/// The monogram layers over the circle as a `SizedBox(n,n).child(Align(CENTER,
/// …))` (the `profile.rs` idiom) — a bare `Align` directly under the `Stack`
/// shrink-wraps to the glyph and lands at the stack origin (top-left), so the
/// tight-sized box is what gives `Align` the bounded constraints it centers
/// within.
fn channel_circle() -> AnyView<HomeState> {
    any(Stack(vec![
        any(fill_box(
            Size::new(AVATAR_SIZE, AVATAR_SIZE),
            CHANNEL_TINT,
            AVATAR_RADIUS,
        )),
        any(SizedBox(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(Align(
            Alignment::CENTER,
            text("#").size(MONOGRAM_SIZE).color(Color::WHITE),
        ))),
    ]))
}

/// The tappable middle-of-row content: name/title, preview, and trailing unread
/// badge — a `GestureDetector` firing `on_tap` (the row navigation).
fn tappable_content<F: Fn(&mut HomeState) + 'static>(
    title: AnyView<HomeState>,
    preview: String,
    unread: u32,
    on_tap: F,
) -> AnyView<HomeState> {
    let column = any(Column(vec![
        title,
        any(SizedBox(None, Some(4.0))),
        any(text(preview).size(LIST_SUBTITLE_SIZE)),
    ]));
    any(GestureDetector(
        FlexView::new(
            Axis::Horizontal,
            vec![flexible(1, column), inflexible(unread_badge(unread))],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
    .on_tap(on_tap))
}

/// The unread badge: a colored pill + count (empty when zero).
fn unread_badge(count: u32) -> AnyView<HomeState> {
    if count == 0 {
        return any(SizedBox(None, None));
    }
    let label = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    any(Padding(
        EdgeInsets::symmetric(4.0, 0.0),
        // SizedBox+Align monogram idiom — see `channel_circle`.
        Stack(vec![
            any(fill_box(
                Size::new(BADGE_W, BADGE_H),
                BADGE_COLOR,
                BADGE_RADIUS,
            )),
            any(SizedBox(Some(BADGE_W), Some(BADGE_H)).child(Align(
                Alignment::CENTER,
                text(label).size(BADGE_TEXT_SIZE).color(Color::WHITE),
            ))),
        ]),
    ))
}

/// The horizontal row layout: leading avatar + tappable content.
fn row_layout(leading: AnyView<HomeState>, content: AnyView<HomeState>) -> AnyView<HomeState> {
    any(Padding(
        EdgeInsets::symmetric(8.0, 10.0),
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(leading),
                inflexible(any(SizedBox(Some(12.0), None))),
                flexible(1, content),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    ))
}

/// A channel roster row, built from extracted state pieces. This is the
/// ListView version of [`channel_row`], used inside the keyed builder closure
/// where the full HomeState cannot be passed as a reference.
fn channel_row_keyed(
    nav: &NavigatorController<HuddleState>,
    controller: &Arc<ChannelsController>,
    toasts: &crate::ui::toast::ToastController,
    _logo: &Option<ImageSource>,
    c: &ChannelItem,
) -> AnyView<HomeState> {
    // Mimic the structure of the original channel_row, but accept the
    // extracted pieces instead of HomeState.
    let leading = any(channel_circle());

    // Title row: the name, plus a lock icon for a private channel.
    let title: AnyView<HomeState> = if c.private {
        any(Row(vec![
            any(text(c.name.clone()).size(LIST_TITLE_SIZE)),
            any(SizedBox(Some(6.0), None)),
            any(icon(icons::LOCK).size(16.0)),
        ]))
    } else {
        any(text(c.name.clone()).size(LIST_TITLE_SIZE))
    };

    let nav_clone = nav.clone();
    let route_id = c.id.clone();
    let content = tappable_content(
        title,
        c.preview.clone(),
        c.unread,
        move |_s: &mut HomeState| {
            let feed_nav = nav_clone.clone();
            let feed_id = route_id.clone();
            nav_clone.push(move || {
                crate::features::messages::presentation::pages::channel_feed::channel_feed(
                    feed_nav.clone(),
                    feed_id.clone(),
                )
            });
        },
    );

    let row = row_layout(leading, content);
    let menu_row = row_with_long_press_menu(row, c.id.clone(), format!("#{}", c.name), c.muted);
    swipe_wrap_keyed(
        menu_row,
        c.id.clone(),
        format!("#{}", c.name),
        controller,
        toasts,
    )
}

/// A DM roster row, built from extracted state pieces. This is the ListView
/// version of [`dm_row`], used inside the keyed builder closure where the full
/// HomeState cannot be passed as a reference.
fn dm_row_keyed(
    nav: &NavigatorController<HuddleState>,
    controller: &Arc<ChannelsController>,
    toasts: &crate::ui::toast::ToastController,
    logo: &Option<ImageSource>,
    d: &DmItem,
) -> AnyView<HomeState> {
    // Mimic the structure of the original dm_row, but accept the extracted
    // pieces instead of HomeState.
    let leading = dm_avatar_keyed(logo, d);

    let title = any(text(d.name.clone()).size(LIST_TITLE_SIZE));
    let nav_clone = nav.clone();
    let route_id = d.id.clone();
    let content = tappable_content(
        title,
        d.preview.clone(),
        d.unread,
        move |_s: &mut HomeState| {
            let feed_nav = nav_clone.clone();
            let feed_id = route_id.clone();
            nav_clone.push(move || {
                crate::features::messages::presentation::pages::channel_feed::channel_feed(
                    feed_nav.clone(),
                    feed_id.clone(),
                )
            });
        },
    );

    let row = row_layout(leading, content);
    let menu_row = row_with_long_press_menu(row, d.id.clone(), d.name.clone(), d.muted);
    swipe_wrap_keyed(menu_row, d.id.clone(), d.name.clone(), controller, toasts)
}

/// A DM avatar built from extracted state pieces (primarily `logo`). This is
/// the ListView version of [`dm_avatar`], used when the full HomeState cannot
/// be passed as a reference.
fn dm_avatar_keyed(_logo: &Option<ImageSource>, d: &DmItem) -> AnyView<HomeState> {
    // A couple of users get a real `Image` avatar; the rest an initials circle.
    let use_image = matches!(d.user_id, 2 | 5) && _logo.is_some();
    let base: AnyView<HomeState> = if use_image {
        let src = _logo.clone().expect("guarded by use_image");
        any(SizedBox(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(Image(src).fit(ImageFit::Cover)))
    } else {
        // SizedBox+Align monogram idiom — see `channel_circle`.
        any(Stack(vec![
            any(fill_box(
                Size::new(AVATAR_SIZE, AVATAR_SIZE),
                avatar_color(d.user_id),
                AVATAR_RADIUS,
            )),
            any(SizedBox(Some(AVATAR_SIZE), Some(AVATAR_SIZE)).child(Align(
                Alignment::CENTER,
                text(d.initials.clone())
                    .size(MONOGRAM_SIZE)
                    .color(Color::WHITE),
            ))),
        ]))
    };

    let dot = any(Align(
        Alignment::new(1.0, 1.0),
        fill_box(Size::new(12.0, 12.0), status_color(d.status), 6.0),
    ));
    let avatar = any(Stack(vec![base, dot]));

    let user_id = d.user_id;
    any(
        GestureDetector(hero(format!("avatar-{user_id}"), avatar)).on_tap(
            move |_s: &mut HomeState| {
                let id = user_id.to_string();
                let nav = _s.nav.clone(); // Capture nav from the state in the closure
                nav.push(move || {
                    crate::features::profile::presentation::pages::profile::profile_screen(
                        id.clone(),
                    )
                });
            },
        ),
    )
}

/// Wrap a row in a [`swipeable_row`](crate::ui::swipeable::swipeable_row):
/// swipe right → archive, left → mute, each raising an undo toast. This
/// version takes extracted pieces (controller, toasts) instead of HomeState.
fn swipe_wrap_keyed(
    row: AnyView<HomeState>,
    id: String,
    label: String,
    controller: &Arc<ChannelsController>,
    toasts: &crate::ui::toast::ToastController,
) -> AnyView<HomeState> {
    let data = controller.data;

    let archive_id = id.clone();
    let archive_label = label.clone();
    let archive_controller = Arc::clone(controller);
    let archive_toasts = toasts.clone();
    let archive_cb = move |_s: &mut HomeState| {
        let handle = archive_controller.clone();
        let op_id = archive_id.clone();
        frust::spawn_local(async move {
            handle.set_archived(op_id, true).await;
        });
        let undo_id = archive_id.clone();
        archive_toasts.show_with_action(format!("Archived {archive_label}"), "Undo", move || {
            channels::set_archived_flag(data, &undo_id, false)
        });
    };

    let mute_id = id;
    let mute_label = label;
    let mute_controller = Arc::clone(controller);
    let mute_toasts = toasts.clone();
    let mute_cb = move |_s: &mut HomeState| {
        let handle = mute_controller.clone();
        let op_id = mute_id.clone();
        frust::spawn_local(async move {
            handle.set_muted(op_id, true).await;
        });
        let undo_id = mute_id.clone();
        mute_toasts.show_with_action(format!("Muted {mute_label}"), "Undo", move || {
            channels::set_muted_flag(data, &undo_id, false)
        });
    };

    any(crate::ui::swipeable::swipeable_row(row)
        .on_swipe_right(ARCHIVE_COLOR, archive_cb)
        .on_swipe_left(MUTE_COLOR, mute_cb))
}

// ---------------------------------------------------------------------------
// Home overlay sheets
// ---------------------------------------------------------------------------

/// The open Home overlay sheet, or an inert zero-size box when nothing is
/// open. `design` (resolved once, at build time, from the live
/// [`use_context::<Theme>`]) picks the invite modal's widget family — see
/// [`show_invite_modal`].
fn home_sheet_overlay(state: &HomeState, design: DesignLanguage) -> AnyView<HomeState> {
    let dismiss = |s: &mut HomeState| s.sheet = HomeSheet::None;
    match state.sheet.clone() {
        HomeSheet::None => any(SizedBox(None, None)),
        HomeSheet::Create => any(sheet(create_channel_form(state)).on_dismiss(dismiss)),
        HomeSheet::RowActions { id, label, muted } => any(sheet(action_menu(row_action_rows(
            state, id, label, muted, design,
        )))
        .on_dismiss(dismiss)),
    }
}

/// The create-channel sheet content: a name field, a private [`switch`]
/// (`icons::LOCK`), and Cancel/Create actions. Create appends the channel to
/// [`ChannelsController`]'s live list (a no-op on an empty/whitespace-only
/// name) and raises a "Created #name" toast; Cancel (and the sheet's own
/// scrim/drag dismiss) discards the draft without touching the roster.
fn create_channel_form(state: &HomeState) -> AnyView<HomeState> {
    let name_field = any(text_input(
        state.create_name.clone(),
        |s: &mut HomeState, next: String| {
            s.create_name = next;
        },
    )
    .placeholder("Channel name"));

    let private_row = any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(any(icon(icons::LOCK).size(18.0))),
            inflexible(any(SizedBox(Some(8.0), None))),
            flexible(1, any(text("Private"))),
            inflexible(any(switch(
                state.create_private,
                |s: &mut HomeState, checked: bool| {
                    s.create_private = checked;
                },
            ))),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center));

    let cancel_btn = any(button("Cancel", |s: &mut HomeState| {
        s.sheet = HomeSheet::None;
    }));
    let create_btn = any(button("Create", |s: &mut HomeState| {
        let trimmed = s.create_name.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        s.controller
            .create_channel(trimmed.clone(), s.create_private);
        s.toasts.show(format!("Created #{trimmed}"));
        s.sheet = HomeSheet::None;
    }));
    let actions_row = any(FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(1, any(SizedBox(None, None))),
            inflexible(cancel_btn),
            inflexible(any(SizedBox(Some(8.0), None))),
            inflexible(create_btn),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center));

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(text("Create a channel").size(18.0))),
                inflexible(any(SizedBox(None, Some(16.0)))),
                inflexible(name_field),
                inflexible(any(SizedBox(None, Some(16.0)))),
                inflexible(private_row),
                inflexible(any(SizedBox(None, Some(20.0)))),
                inflexible(actions_row),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch),
    ))
}

/// The row long-press menu: Mute/Unmute, Archive, Invite people, Cancel.
/// Mute/Archive reuse [`ChannelsController`]'s existing `set_muted`/
/// `set_archived` ops + an undo toast — the exact [`swipe_wrap`] shape, so a
/// long-press and a swipe converge on the same controller call. "Invite
/// people" is the one entry point into [`show_invite_modal`] (see
/// the [module docs](self)).
fn row_action_rows(
    state: &HomeState,
    id: String,
    label: String,
    muted: bool,
    design: DesignLanguage,
) -> Vec<AnyView<HomeState>> {
    let data = state.controller.data;

    let mute_id = id.clone();
    let mute_label = label.clone();
    let mute_icon = if muted {
        icons::NOTIFICATIONS
    } else {
        icons::VOLUME_OFF
    };
    let mute_text = if muted { "Unmute" } else { "Mute" };
    let mute_row = sheet_action_row(mute_icon, mute_text, move |s: &mut HomeState| {
        s.sheet = HomeSheet::None;
        let next = !muted;
        let handle = s.controller.clone();
        let op_id = mute_id.clone();
        frust::spawn_local(async move {
            handle.set_muted(op_id, next).await;
        });
        let undo_id = mute_id.clone();
        let verb = if next { "Muted" } else { "Unmuted" };
        s.toasts
            .show_with_action(format!("{verb} {mute_label}"), "Undo", move || {
                channels::set_muted_flag(data, &undo_id, !next)
            });
    });

    let archive_id = id.clone();
    let archive_label = label.clone();
    let archive_row = sheet_action_row(icons::ARCHIVE, "Archive", move |s: &mut HomeState| {
        s.sheet = HomeSheet::None;
        let handle = s.controller.clone();
        let op_id = archive_id.clone();
        frust::spawn_local(async move {
            handle.set_archived(op_id, true).await;
        });
        let undo_id = archive_id.clone();
        let undo_label = archive_label.clone();
        s.toasts
            .show_with_action(format!("Archived {undo_label}"), "Undo", move || {
                channels::set_archived_flag(data, &undo_id, false)
            });
    });

    let nav = state.nav.clone();
    let invite_row = sheet_action_row(icons::GROUP, "Invite people", move |s: &mut HomeState| {
        s.sheet = HomeSheet::None;
        show_invite_modal(&nav, design);
    });

    let cancel_row = sheet_action_row(icons::CLOSE, "Cancel", |s: &mut HomeState| {
        s.sheet = HomeSheet::None;
    });

    vec![mute_row, archive_row, invite_row, cancel_row]
}

/// Push the invite confirmation modal onto the outer app navigator: `dialog`
/// on Material 3, `cupertino_alert` on Cupertino (see the [module
/// docs](self)).
///
/// Neither builds an explicit Cancel action: both `show_dialog` and
/// `show_cupertino_alert` already auto-wire a scrim tap to a plain dismiss (an
/// empty [`PopResult`], per each widget's own doc comment) — tapping outside
/// the panel *is* the cancel affordance, so a second, redundant Cancel button
/// would just duplicate it. Confirming with the one "Send" action toasts
/// "Invites sent (mock)"; a scrim-tap cancel does nothing.
fn show_invite_modal(nav: &NavigatorController<HuddleState>, design: DesignLanguage) {
    match design {
        // Glyph has no modal chrome baseline yet —
        // falls through to the Material3 arm for now.
        DesignLanguage::Material3 | DesignLanguage::Glyph => {
            let confirm_nav = nav.clone();
            show_dialog(
                nav,
                move || {
                    let confirm = confirm_nav.clone();
                    dialog()
                        .title("Invite people")
                        .body("Send invites to this workspace? (mock)")
                        .action(any(button("Send", move |_s: &mut HuddleState| {
                            confirm.pop_with_result(PopResult::of(true));
                        })))
                },
                |s: &mut HuddleState, result: PopResult| {
                    if result.take::<bool>() == Some(true) {
                        s.toasts.show("Invites sent (mock)");
                    }
                },
            );
        }
        DesignLanguage::Cupertino => {
            show_cupertino_alert(
                nav,
                "Invite people",
                Some("Send invites to this workspace? (mock)".to_string()),
                vec![action("Send")],
                |s: &mut HuddleState, result: PopResult| {
                    if result.take::<usize>() == Some(0) {
                        s.toasts.show("Invites sent (mock)");
                    }
                },
            );
        }
        _ => {
            // external design systems (DesignLanguage::Custom) fall back to Material chrome here
            let confirm_nav = nav.clone();
            show_dialog(
                nav,
                move || {
                    let confirm = confirm_nav.clone();
                    dialog()
                        .title("Invite people")
                        .body("Send invites to this workspace? (mock)")
                        .action(any(button("Send", move |_s: &mut HuddleState| {
                            confirm.pop_with_result(PopResult::of(true));
                        })))
                },
                |s: &mut HuddleState, result: PopResult| {
                    if result.take::<bool>() == Some(true) {
                        s.toasts.show("Invites sent (mock)");
                    }
                },
            );
        }
    }
}
