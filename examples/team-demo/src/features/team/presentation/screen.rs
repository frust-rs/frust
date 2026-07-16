//! [`TeamScreen`] — the team feature's single screen, rewritten from the Leptos
//! twin's `pages.rs`/`components.rs` as a ForgeKit [`Component`] over the
//! `clean-signals-forgekit` glue (`use_controller` / `use_failure_listener` /
//! `async_view`).
//!
//! The Leptos twin was a fine-grained reactive `view!` tree (`<Show>`/`<For>`);
//! ForgeKit re-runs the whole `build` on every tracked-signal change (coarse-
//! grained — see `clean_signals_forgekit`'s crate docs), so the presentation is
//! a plain view-building function over already-read signal snapshots. The
//! controller, domain, and data layers are ported unchanged; only this file is
//! a rewrite (the twin's `pages.rs` view macros do not translate).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use clean_signals::Failure;
use forgekit::{
    AnyView, Axis, Button, Column, Component, FlexChild, FlexView, Get, Row, RwSignal, Set,
    SizedBox, any, component, keyed, scroll_view, text, text_input,
};

use crate::failure::TeamFailure;
use crate::features::team::domain::entities::Member;
use crate::features::team::domain::repositories::TeamRepository;
use crate::features::team::presentation::controllers::TeamController;
use clean_signals_forgekit::{async_view, use_controller, use_failure_listener};

/// The team screen's retained [`Component`] state.
///
/// Per `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions, only the
/// fields that need cross-`build` observation are signals: `banner` is an
/// `RwSignal` because a *background* failure listener (registered in `init`,
/// firing outside `build`) writes it, and the controller owns its own signals.
/// The two draft fields are plain data — they only ever change inside a widget
/// event handler that already has `&mut TeamState`.
pub struct TeamState {
    /// The shared controller handle (built once in `init`, disposed on teardown).
    pub controller: Arc<TeamController>,
    /// The failure banner text, `Some` while a failure is being surfaced.
    ///
    /// A signal (not a plain `Option<String>`) because the
    /// [`use_failure_listener`] callback that sets it runs *outside* any
    /// `build` — a background subscription, not a widget event — so it needs
    /// cross-tree reactivity to wake the next frame. This banner-via-signal
    /// pattern is established HERE (first use in the ForgeKit examples): a
    /// component-scoped `RwSignal<Option<String>>` fed by a failure listener
    /// and rendered as a dismissible banner arm.
    pub banner: RwSignal<Option<String>>,
    /// The search field's controlled draft text (the notes draft pattern): the
    /// `TextInput`'s canonical value lives here so a per-frame reconcile does
    /// not fight in-progress typing. `on_change` mirrors it into the
    /// controller's `query` signal, which the `filtered` memo reads.
    pub query_draft: String,
    /// Per-row rename drafts, keyed by member id, seeded from the member's name.
    ///
    /// Each row's rename `TextInput` is a controlled component; without a
    /// per-row draft here the every-frame reconcile would overwrite what the
    /// user is typing with the canonical `Member.name` (see the notes demo's
    /// draft pattern and RESEARCH §3's TextInput note).
    pub name_drafts: HashMap<String, String>,
}

/// The handles a headless test observes: the controller (for its signals and to
/// drive `rename`/`load` directly) and the banner signal (to assert the failure
/// banner arm). Published by [`TeamScreen::init`] when a spy is attached.
pub struct TeamHandles {
    pub controller: Arc<TeamController>,
    pub banner: RwSignal<Option<String>>,
}

/// A test spy: [`TeamScreen::init`] publishes its handles here so a headless
/// test can observe the controller and banner across pumped frames. `None` in
/// production.
pub type TeamSpy = Arc<Mutex<Option<TeamHandles>>>;

/// The team list screen. `repo` is injected by the composition root
/// (constructor injection, never looked up by the screen itself) — the same DI
/// shape as the Leptos twin's `TeamPage(repo)`.
pub struct TeamScreen {
    repo: Arc<dyn TeamRepository + Send + Sync>,
    spy: Option<TeamSpy>,
}

impl TeamScreen {
    /// A production screen (no test spy).
    pub fn new(repo: Arc<dyn TeamRepository + Send + Sync>) -> Self {
        Self { repo, spy: None }
    }

    /// A screen that publishes its handles to `spy` on `init` (headless test).
    pub fn with_spy(repo: Arc<dyn TeamRepository + Send + Sync>, spy: TeamSpy) -> Self {
        Self {
            repo,
            spy: Some(spy),
        }
    }
}

impl Component for TeamScreen {
    type State = TeamState;

    fn init(&self) -> TeamState {
        // The component's reactive `Owner` is ambient here, so `use_controller`'s
        // `on_cleanup` binds disposal to *this* component's teardown, and
        // `use_failure_listener`'s subscription is dropped on teardown too.
        let repo = Arc::clone(&self.repo);
        let controller =
            use_controller::<TeamController, TeamFailure>(move || TeamController::new(repo));

        // Surface failures once, scoped to this screen, into a banner signal.
        // See `TeamState::banner` — this failure-listener-into-a-signal is the
        // banner pattern established by this example.
        let banner: RwSignal<Option<String>> = RwSignal::new(None);
        use_failure_listener(controller.failures(), move |f: TeamFailure| {
            banner.set(Some(f.user_message()));
        });

        // Kick off the initial load on the UI-thread local task queue. `load()`
        // is disposal-gated inside `ControllerCore::run_into`, so a screen torn
        // down mid-load never touches its freed signals (no try_get_value guard
        // needed, unlike the Leptos twin's `StoredValue`).
        let handle = Arc::clone(&controller);
        forgekit::spawn_local(async move {
            handle.load().await;
        });

        if let Some(spy) = &self.spy {
            *spy.lock().unwrap_or_else(|e| e.into_inner()) = Some(TeamHandles {
                controller: Arc::clone(&controller),
                banner,
            });
        }

        TeamState {
            controller,
            banner,
            query_draft: String::new(),
            name_drafts: HashMap::new(),
        }
    }

    fn build(&self, state: &mut TeamState) -> AnyView<TeamState> {
        // Tracked reads: reading these signals inside `build` subscribes the
        // shell's frame-tracking scope, so a later controller/listener write
        // wakes the next frame (see clean_signals_forgekit's "Coarse-grained
        // reactivity model"). `members` drives the async_view arm below.
        let members_snapshot = state.controller.members.get();
        let banner_msg = state.banner.get();
        let query_draft = state.query_draft.clone();
        // `Memo` is `Copy`; the closure re-reads `filtered.get()` (also tracked)
        // so the list narrows when either `query` or `members` changes.
        let filtered = state.controller.filtered;
        let name_drafts = state.name_drafts.clone();

        let mut children: Vec<AnyView<TeamState>> = Vec::new();
        children.push(any(text("Team").size(32.0)));

        // Banner arm: a failure message plus a Dismiss button that clears the
        // component-state banner signal.
        if let Some(msg) = banner_msg {
            children.push(any(Row(vec![
                any(text(msg)),
                any(SizedBox(Some(8.0), None)),
                any(Button("Dismiss", |st: &mut TeamState| {
                    st.banner.set(None);
                })),
            ])));
        }

        // Search field: controlled by `query_draft`; `on_change` updates the
        // draft AND the controller's `query` signal the `filtered` memo reads.
        children.push(any(text_input(
            query_draft,
            |st: &mut TeamState, v: String| {
                st.query_draft = v.clone();
                st.controller.query.set(v);
            },
        )
        .placeholder("Search by name or role")));

        // The member list through its load lifecycle. `members_snapshot` selects
        // the arm; the Data arm renders the *filtered* list. Reloading folds to
        // Data and a stale error drops its last-known-good value — the v0 glue
        // compromises documented in `clean_signals_forgekit::async_view` (this
        // screen never triggers either: it has no refresh path).
        let body = async_view(
            members_snapshot,
            || any(text("Loading team…")),
            move |_all: Vec<Member>| {
                let rows: Vec<FlexChild<TeamState>> = filtered
                    .get()
                    .into_iter()
                    .map(|m| {
                        // Seed each row's rename draft from the current draft map
                        // (what the user has typed) falling back to the canonical
                        // name — the notes draft pattern (RESEARCH §3).
                        let draft = name_drafts
                            .get(&m.id)
                            .cloned()
                            .unwrap_or_else(|| m.name.clone());
                        let id = m.id.clone();
                        // Keyed by member id via FlexView — `Column` cannot key
                        // (it hardcodes `key: None`); an identity-keyed row keeps
                        // the right rename-input widget across a filter reorder
                        // (RESEARCH §3, docs/ARCHITECTURE.md's `ChildKey`).
                        keyed(id, member_row(m, draft))
                    })
                    .collect();
                any(scroll_view(FlexView::new(Axis::Vertical, rows)))
            },
            // Error arm: the message plus a Retry button that re-kicks the load.
            // Faithful to the Leptos twin — its error state also permits a
            // re-attempt via reload; kept minimal (no stale-data slot; see the
            // v0 note above).
            |failure: TeamFailure| {
                any(Row(vec![
                    any(text(failure.user_message())),
                    any(SizedBox(Some(8.0), None)),
                    any(Button("Retry", |st: &mut TeamState| {
                        let handle = Arc::clone(&st.controller);
                        forgekit::spawn_local(async move {
                            handle.load().await;
                        });
                    })),
                ]))
            },
        );
        children.push(body);

        any(Column(children))
    }
}

/// One member row: name / role / email texts plus an inline rename `TextInput`.
///
/// A plain view-building function (the twin's `member_row`), not a `Component` —
/// it has no retained state of its own; the rename draft lives in the parent
/// [`TeamState::name_drafts`], seeded into `draft` here.
fn member_row(member: Member, draft: String) -> FlexView<TeamState> {
    let id_for_change = member.id.clone();
    let id_for_submit = member.id.clone();
    Row(vec![
        any(text(member.name.clone())),
        any(SizedBox(Some(8.0), None)),
        any(text(member.role.clone())),
        any(SizedBox(Some(8.0), None)),
        any(text(member.email.clone())),
        any(SizedBox(Some(8.0), None)),
        any(text_input(draft, move |st: &mut TeamState, v: String| {
            st.name_drafts.insert(id_for_change.clone(), v);
        })
        .placeholder("Rename")
        .on_submit(move |st: &mut TeamState, v: String| {
            let id = id_for_submit.clone();
            let handle = Arc::clone(&st.controller);
            // Drive the rename through the controller (routed through
            // `ControllerCore::run` — retry/activity/failure). Validation
            // failures surface on the failure sink → the banner listener.
            forgekit::spawn_local(async move {
                handle.rename(id, v).await;
            });
        })),
    ])
}

/// Hosts [`TeamScreen`] under a real component [`Owner`] with a default
/// in-memory backend — the app root `forgekit::app!` binds. Mirrors the Leptos
/// twin's composition root (`main.rs`) injecting the repository.
pub fn team_screen_with_default_repo() -> AnyView<()> {
    use crate::features::team::data::InMemoryTeamRepo;
    use std::time::Duration;

    // A latency + one seeded failure so the desktop/mobile run visibly exercises
    // the retry path (the first load attempt fails, the RetryPolicy retries).
    let repo: Arc<dyn TeamRepository + Send + Sync> =
        Arc::new(InMemoryTeamRepo::new(Duration::from_millis(400), 1));
    any(component(TeamScreen::new(repo)))
}
