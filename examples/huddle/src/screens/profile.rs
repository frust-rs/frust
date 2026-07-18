//! Profile screen (wave-2 task 06) — an avatar, the committed profile, editable
//! fields, and a save button whose validation failures surface as a banner.
//!
//! A [`Component`] hosting a [`ProfileController`](crate::profile_domain) via
//! `use_controller` (the `TeamScreen` seam). The three edit buffers are plain
//! `Component::State` strings (they only ever change inside a widget event
//! handler that already holds `&mut ProfileState`, per
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions); the failure
//! `banner` is an `RwSignal` because a background failure listener writes it
//! from outside `build` — the same banner pattern `TeamScreen` establishes.
//!
//! Reached from the shell's app-bar "Profile" button (`router.push("/profile")`,
//! wired in task 02's `shell.rs`). The Save button branches Material/Cupertino
//! like the shell chrome, and the visible failure path (blank name / malformed
//! email → banner, then fix + re-save clears it) is acceptance criterion 3.

use std::sync::Arc;

use clean_signals::Failure;
use forgekit::{
    AnyView, Button, Column, Component, DesignLanguage, EdgeInsets, Get, GetUntracked, Image,
    ImageFit, Padding, Row, RwSignal, Set, SizedBox, Theme, any, component, cupertino_button,
    scroll_view, text, text_input, use_context,
};

use crate::ShellState;
use crate::failure::TeamFailure;
use crate::profile_domain::ProfileController;
use clean_signals_forgekit::{use_controller, use_failure_listener};

/// The profile screen's route entry point: a `Component` under `ShellState`.
pub fn profile_screen() -> AnyView<ShellState> {
    any(component(ProfileScreen))
}

/// The profile screen `Component`. Stateless configuration.
struct ProfileScreen;

/// Retained state: the hosted controller, the failure banner signal, and the
/// three controlled edit buffers.
struct ProfileState {
    controller: Arc<ProfileController>,
    /// The validation-failure banner, `Some` while a failure is surfaced. A
    /// signal because the `use_failure_listener` callback that sets it runs
    /// outside any `build` (the `TeamScreen` banner pattern).
    banner: RwSignal<Option<String>>,
    name_draft: String,
    role_draft: String,
    email_draft: String,
}

impl Component for ProfileScreen {
    type State = ProfileState;

    fn init(&self) -> ProfileState {
        let controller = use_controller::<ProfileController, TeamFailure>(ProfileController::new);

        // Surface validation failures into the banner signal, scoped to this
        // screen (disposed with the component's owner on teardown).
        let banner: RwSignal<Option<String>> = RwSignal::new(None);
        use_failure_listener(controller.failures(), move |f: TeamFailure| {
            banner.set(Some(f.user_message()));
        });

        // Seed the edit buffers from the committed profile.
        let seed = controller.profile.get_untracked();
        ProfileState {
            controller,
            banner,
            name_draft: seed.name,
            role_draft: seed.role,
            email_draft: seed.email,
        }
    }

    fn build(&self, state: &mut ProfileState) -> AnyView<ProfileState> {
        // Tracked reads: a successful save writes `profile`, waking the frame.
        let profile = state.controller.profile.get();
        let banner_msg = state.banner.get();
        let name_draft = state.name_draft.clone();
        let role_draft = state.role_draft.clone();
        let email_draft = state.email_draft.clone();

        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design = theme.design_language;

        let mut children: Vec<AnyView<ProfileState>> = Vec::new();
        children.push(any(text("Profile").size(32.0)));

        // Avatar (decode-once handle, re-passed each frame) sized into a fixed
        // box so it does not dominate the column.
        children.push(any(SizedBox(Some(96.0), Some(96.0))
            .child(any(Image(profile.avatar.clone()).fit(ImageFit::Contain)))));

        // Committed profile summary (updates on a successful save).
        children.push(any(
            text(format!("{} — {}", profile.name, profile.role)).size(18.0)
        ));
        children.push(any(text(profile.email.clone()).size(14.0)));
        children.push(any(SizedBox(None, Some(12.0))));

        // Failure banner arm (the visible failure path) + a Dismiss button.
        if let Some(msg) = banner_msg {
            children.push(any(Row(vec![
                any(text(msg)),
                any(SizedBox(Some(8.0), None)),
                any(Button("Dismiss", |st: &mut ProfileState| {
                    st.banner.set(None);
                })),
            ])));
        }

        // Editable fields (controlled by the plain draft buffers).
        children.push(any(text_input(
            name_draft,
            |st: &mut ProfileState, v: String| {
                st.name_draft = v;
            },
        )
        .placeholder("Name")));
        children.push(any(text_input(
            role_draft,
            |st: &mut ProfileState, v: String| {
                st.role_draft = v;
            },
        )
        .placeholder("Role")));
        children.push(any(text_input(
            email_draft,
            |st: &mut ProfileState, v: String| {
                st.email_draft = v;
            },
        )
        .placeholder("Email")));
        children.push(any(SizedBox(None, Some(12.0))));

        // Save: clears any prior banner optimistically, then drives the
        // controller's validate+save. A rejected save re-raises the banner via
        // the failure listener; a fixed field + re-save clears it (recovery).
        let save = |st: &mut ProfileState| {
            st.banner.set(None);
            let handle = Arc::clone(&st.controller);
            let name = st.name_draft.clone();
            let role = st.role_draft.clone();
            let email = st.email_draft.clone();
            forgekit::spawn_local(async move {
                handle.save(name, role, email).await;
            });
        };
        children.push(match design {
            DesignLanguage::Material3 => any(Button("Save", save)),
            DesignLanguage::Cupertino => any(cupertino_button("Save", save)),
        });

        any(scroll_view(Padding(
            EdgeInsets::all(16.0),
            any(Column(children)),
        )))
    }
}
