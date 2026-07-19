//! Workspace switcher (`/workspace-switcher`) — a top drawer.
//!
//! # Transition note (skeleton, superseded by this task)
//!
//! The skeleton (task 10) routed this page through the navigator's normal
//! (opaque) transition, deferring a slide-down top-drawer transition to Phase
//! C: "the framework has no `SlideDown` preset — the transparent push
//! arrives instantly, your panel animates itself in." This task delivers
//! that: the panel drives its own entrance via an app-level
//! [`AnimationController`] (see [`entrance_progress`]) rather than a
//! navigator `TransitionSpec`, independent of whatever transition (or none)
//! the page arrives under.
//!
//! # Scrim (widget-owned, per the `Dialog`/`BottomSheet` precedent)
//!
//! `forgekit-widgets`' `Dialog`/`BottomSheet` each paint their own scrim
//! directly in their retained `Widget::paint` (the navigator itself paints
//! no scrim — see `dialog.rs`'s module docs) — a capability this file, built
//! only from the public `forgekit` facade (no `forgekit-core`/`-widgets`
//! dependency, no raw `PaintScene`/`Widget` access — see this crate's
//! `Cargo.toml`), cannot replicate exactly. The nearest equivalent
//! composable from existing widgets is a full-bleed, interactive
//! [`filled_card`] behind the panel: [`SizedBox`]'s "request bigger than
//! anything realistic, let it clamp to whatever space is actually available"
//! contract (see [`FILL`]) is what makes it full-bleed without needing to
//! know the window's real size up front. It is an **opaque** theme surface
//! tone, not a true alpha-blended dim (`Card` has no alpha/translucency
//! knob), and its corners pick up `filled_card`'s uniform corner radius
//! rather than staying perfectly square at the window edge — both accepted,
//! documented simplifications of the real thing.
//!
//! # Drag-up-to-dismiss (task 21)
//!
//! Since Home's task 21 already re-added a `forgekit-core` production
//! dependency to this crate (`ui/swipeable.rs`, `screens/home.rs`'s escape
//! hatches, and `ui/sheet.rs`'s bottom sheet — see this crate's `Cargo.toml`),
//! the panel now also wraps in [`crate::ui::sheet::drag_up_dismiss`]: a
//! vertical drag upward past a threshold pops the route, mirroring
//! [`SheetView`](crate::ui::sheet::SheetView)'s downward drag-dismiss with the
//! direction inverted. The scrim tap below still pops too — the two dismiss
//! paths are independent and don't interact.
//!
//! # Rounded bottom corners
//!
//! The spec calls for a panel with rounded *bottom* corners only. The public
//! `Card` widgets only expose a uniform radius on all four corners (no
//! per-corner override), so the panel below is a uniformly-rounded card, not
//! a bottom-only one — the same kind of simplification as the scrim's, for
//! the same reason (no raw path/shape primitive reachable from facade-only
//! app code).

use std::cell::Cell;
use std::time::{Duration, Instant};

use forgekit::{
    Align, Alignment, AnimationController, AnyView, Column, Curve, EdgeInsets, FrameTime,
    GestureDetector, Get, GetUntracked, Padding, Row, RwSignal, Set, SizedBox, Stack, any,
    filled_card, text,
};

use crate::HuddleState;
use crate::ui::sheet::drag_up_dismiss;

/// One row in the switcher: an initials tile, a name, and an unread/active
/// hint. Mock data local to this screen — there is no "workspace" concept in
/// the shared [`crate::mock`] dataset (a single-workspace app per PLAN.md),
/// so multi-workspace switching here is mock-only (see [`workspace_row`]'s
/// tap handler): no real multi-workspace state exists anywhere else in the
/// app.
struct Workspace {
    name: &'static str,
    initials: &'static str,
    active: bool,
    unread: u32,
}

const WORKSPACES: [Workspace; 3] = [
    Workspace {
        name: "Huddle HQ",
        initials: "HH",
        active: true,
        unread: 0,
    },
    Workspace {
        name: "Forge Labs",
        initials: "FL",
        active: false,
        unread: 3,
    },
    Workspace {
        name: "Weekend Crew",
        initials: "WC",
        active: false,
        unread: 12,
    },
];

/// A dimension bigger than any realistic window. [`SizedBox`] clamps a
/// requested width/height into the constraints it's actually laid out under
/// (see its doc comment), so requesting this always resolves to "fill
/// whatever space is really available" without this screen needing to know
/// the live window size.
const FILL: f64 = 10_000.0;

/// The panel's slide-in travel distance, in logical px — a fixed demo value,
/// not measured from the panel's real laid-out height (app-level view code,
/// built only from the `forgekit` facade, has no read-back-after-layout
/// hook). Chosen comfortably smaller than the panel's expected content
/// height so the animated top inset (see [`workspace_drawer_screen`]) never
/// goes negative.
const ENTRANCE_SLIDE_DISTANCE: f64 = 64.0;

/// The entrance animation's total duration.
const ENTRANCE_DURATION: Duration = Duration::from_millis(220);

thread_local! {
    /// The entrance-progress signal, created once per process (see
    /// [`entrance_progress`]). `workspace_drawer_screen` is a plain page
    /// builder re-invoked by the navigator on *every* rebuild (see
    /// `forgekit_widgets::nav::navigator`'s `PageBuilder` doc) — caching the
    /// signal here is what lets repeated calls observe the same animating
    /// value instead of restarting it from zero every frame. `RwSignal` is a
    /// cheap `Copy` handle, so a `Cell` is enough (no `RefCell` needed).
    ///
    /// **Self-healing (task 22 hardening):** an `RwSignal` is owned by the
    /// reactive `Owner` live when it was created; a headless test that disposes
    /// its ambient owner and then re-enters (or a reused test thread) leaves this
    /// cache holding a *disposed* signal whose next `get`/`set` panics. So
    /// [`entrance_progress`] validates the cached handle with
    /// `try_get_untracked()` and recreates it when disposal is detected, rather
    /// than trusting the cache blindly. This is the flake task 21's report
    /// flagged.
    static ENTRANCE: Cell<Option<RwSignal<f64>>> = const { Cell::new(None) };
}

/// The `0.0..=1.0` entrance progress, animated once per process by a
/// background timer loop — the same `forgekit::spawn` + sleep + signal-write
/// pattern `crate::ui::toast`'s auto-dismiss timer already uses (see its
/// module docs), reused here because a plain page-builder function (this
/// one) has no per-frame `PaintCtx`/`request_frame` hook to drive an
/// [`AnimationController`] the way a retained framework `Widget` would (see
/// `docs/ARCHITECTURE.md`'s Frame pipeline) — that hook only exists inside
/// `forgekit-core`/`-widgets`, which this crate's production code doesn't
/// depend on (see this crate's `Cargo.toml`).
fn entrance_progress() -> RwSignal<f64> {
    ENTRANCE.with(|cell| {
        if let Some(sig) = cell.get()
            && sig.try_get_untracked().is_some()
        {
            // A live, still-owned signal — reuse it. A disposed one (its Owner
            // gone) returns `None` and falls through to be recreated below.
            return sig;
        }
        let sig = RwSignal::new(0.0);
        cell.set(Some(sig));

        forgekit::spawn(async move {
            let mut controller =
                AnimationController::new(ENTRANCE_DURATION).with_curve(Curve::EaseOut);
            controller.animate_to(1.0);
            controller.advance(FrameTime::ZERO); // seed the clock (zero delta)

            let mut elapsed_ns: u64 = 0;
            let mut last = Instant::now();
            while controller.is_animating() {
                clean_signals::time::sleep(Duration::from_millis(16)).await;
                let now = Instant::now();
                elapsed_ns = elapsed_ns.saturating_add(now.duration_since(last).as_nanos() as u64);
                last = now;
                controller.advance(FrameTime::from_nanos(elapsed_ns));
                sig.set(controller.value_clamped());
            }
            sig.set(1.0);
        });

        sig
    })
}

/// The workspace-switcher page: a full-bleed scrim behind a top-anchored,
/// full-width drawer panel that slides down into place on first appearance.
pub fn workspace_drawer_screen() -> AnyView<HuddleState> {
    // Tracked read: a write from the entrance ticker wakes this page's own
    // rebuild until the animation settles at `1.0`.
    let progress = entrance_progress().get().clamp(0.0, 1.0);
    let top_offset = (1.0 - progress) * ENTRANCE_SLIDE_DISTANCE;

    let scrim: AnyView<HuddleState> = any(GestureDetector(SizedBox(Some(FILL), Some(FILL)))
        .on_tap(|s: &mut HuddleState| {
            s.nav.router().pop();
        }));

    let rows: Vec<AnyView<HuddleState>> = WORKSPACES.iter().map(workspace_row).collect();
    // Drag-up-to-dismiss (task 21): a vertical drag upward on the panel past
    // its own threshold pops the route, mirroring the scrim tap below —
    // `ui::sheet::drag_up_dismiss` is the sheet's downward drag-dismiss
    // mechanics inverted (see its doc comment).
    let card = drag_up_dismiss(filled_card(Column(rows)))
        .on_dismiss(|s: &mut HuddleState| s.nav.router().pop());
    let panel_content = SizedBox(Some(FILL), None).child(card);
    let panel_slid = Padding(
        EdgeInsets {
            left: 0.0,
            top: -top_offset,
            right: 0.0,
            bottom: 0.0,
        },
        panel_content,
    );
    let panel: AnyView<HuddleState> = any(Align(Alignment::new(0.0, -1.0), panel_slid));

    any(Stack(vec![scrim, panel]))
}

/// One tappable workspace row: an initials tile, name, and a trailing
/// checkmark (active) or unread count.
fn workspace_row(ws: &Workspace) -> AnyView<HuddleState> {
    let name = ws.name;
    let active = ws.active;
    let unread = ws.unread;

    let mut cells: Vec<AnyView<HuddleState>> = vec![
        initials_tile(ws.initials, 40.0, 16.0),
        any(SizedBox(Some(12.0), None)),
        any(text(name).size(16.0)),
    ];
    if active {
        cells.push(any(SizedBox(Some(8.0), None)));
        cells.push(any(text("\u{2713}").size(16.0))); // check mark: the active workspace
    } else if unread > 0 {
        cells.push(any(SizedBox(Some(8.0), None)));
        cells.push(any(text(format!("{unread}")).size(14.0)));
    }

    any(Padding(
        EdgeInsets::symmetric(16.0, 12.0),
        GestureDetector(Row(cells)).on_tap(move |s: &mut HuddleState| {
            // Tapping the already-active workspace does nothing (see the
            // module docs: no real multi-workspace state exists to switch).
            if active {
                return;
            }
            s.toasts.show(format!("Switched to {name}"));
            s.nav.router().pop();
        }),
    ))
}

/// A rounded initials tile — see `crate::screens::profile`'s copy of this
/// helper for why it's a uniformly-rounded square rather than a true circle.
/// Kept as a small, separately-owned duplicate rather than a shared import so
/// this screen and `screens::profile` stay disjoint files per
/// `src/README-phase-c.md`'s Phase C contract.
fn initials_tile<State: 'static>(initials: &str, tile_size: f64, font_size: f32) -> AnyView<State> {
    any(filled_card(
        SizedBox(Some(tile_size), Some(tile_size))
            .child(Align(Alignment::CENTER, text(initials).size(font_size))),
    ))
}
