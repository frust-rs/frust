//! The one priority order across every workbench-blocking modal (G3): the
//! six modal fields on [`AppState`] used to be walked by two independently
//! hand-maintained if/else chains — `crate::runner::translate_key`'s key
//! routing and `crate::ui::render`'s suppression/overlay dispatch — that could
//! silently drift apart (a modal added to one chain and forgotten in the
//! other). [`ActiveModal`] plus [`AppState::active_modal`] make the priority
//! order a single, compiler-checked source: both call sites now match
//! exhaustively on this enum, so a seventh modal (or a reordered priority)
//! that isn't handled in one of the two call sites fails to compile instead
//! of silently misbehaving at runtime.

use std::path::Path;

use super::add_plugin::AddPluginDialog;
use super::bootstrap::BootstrapWizard;
use super::build_launcher::BuildLauncher;
use super::create_wizard::CreateWizard;
use super::palette::Palette;
use super::run_config::RunConfig;
use super::state::AppState;

/// The single workbench-blocking modal currently capturing input and
/// suppressing background mouse regions, if any — in priority order (highest
/// first): [`Self::CreateWizard`] can appear over either top-level screen
/// (see `crate::ui::render`); the rest are workbench-only and mutually
/// exclusive by construction (each is opened by a key/click path gated on no
/// higher-priority modal already being open — see `translate_key`'s previous
/// chain, now `AppState::active_modal`'s match arms).
#[derive(Debug, Clone, Copy)]
pub enum ActiveModal<'a> {
    /// The fuzzy command palette (`state.palette`) — top priority, opened from
    /// the base layer of either top-level screen (`Ctrl+P` / `:`).
    Palette(&'a Palette),
    /// The create-project wizard (`state.create_wizard`) — can appear over the
    /// welcome splash or the workbench.
    CreateWizard(&'a CreateWizard),
    /// The toolchain bootstrap wizard (`state.bootstrap_wizard`) — like the
    /// create wizard, it can appear over either top-level screen (the
    /// titlebar chip shows on both).
    Bootstrap(&'a BootstrapWizard),
    /// The Add Plugin dialog (`state.add_plugin`) — like the wizards above, it
    /// can appear over either top-level screen (though it needs an open project
    /// to reach — see `crate::engine::update`'s `OpenAddPlugin`).
    AddPlugin(&'a AddPluginDialog),
    /// The keyboard/help overlay (`state.help_open`, `?` — T05 / D5) — like
    /// the wizards above, reachable from either top-level screen (both
    /// status bars carry the `? help` hint).
    HelpOverlay,
    /// The run-config modal (`state.run_config`).
    RunConfig(&'a RunConfig),
    /// The titlebar project switcher (`state.project_switcher_open`).
    ProjectSwitcher,
    /// The doctor panel (`state.doctor_panel_open`).
    DoctorPanel,
    /// The build-launcher modal (`state.build_launcher`).
    BuildLauncher(&'a BuildLauncher),
    /// The clean-confirm dialog (`state.clean_confirm`), carrying the target
    /// project root.
    CleanConfirm(&'a Path),
}

impl AppState {
    /// The one currently-active modal, in priority order, or `None` if the
    /// workbench chrome itself has input focus. This is the single priority
    /// source `translate_key`'s modal routing and `ui::render`'s
    /// suppression/overlay dispatch both match on exhaustively — see the
    /// module doc comment.
    pub fn active_modal(&self) -> Option<ActiveModal<'_>> {
        if let Some(palette) = &self.palette {
            Some(ActiveModal::Palette(palette))
        } else if let Some(wizard) = &self.create_wizard {
            Some(ActiveModal::CreateWizard(wizard))
        } else if let Some(wizard) = &self.bootstrap_wizard {
            Some(ActiveModal::Bootstrap(wizard))
        } else if let Some(dialog) = &self.add_plugin {
            Some(ActiveModal::AddPlugin(dialog))
        } else if self.help_open {
            Some(ActiveModal::HelpOverlay)
        } else if let Some(modal) = &self.run_config {
            Some(ActiveModal::RunConfig(modal))
        } else if self.project_switcher_open {
            Some(ActiveModal::ProjectSwitcher)
        } else if self.doctor_panel_open {
            Some(ActiveModal::DoctorPanel)
        } else if let Some(launcher) = &self.build_launcher {
            Some(ActiveModal::BuildLauncher(launcher))
        } else {
            self.clean_confirm.as_deref().map(ActiveModal::CleanConfirm)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::create_wizard::CreateWizard;

    #[test]
    fn no_modal_open_is_none() {
        let state = AppState::default();
        assert!(state.active_modal().is_none());
    }

    #[test]
    fn each_modal_open_alone_is_returned() {
        let state = AppState {
            project_switcher_open: true,
            ..AppState::default()
        };
        assert!(matches!(
            state.active_modal(),
            Some(ActiveModal::ProjectSwitcher)
        ));

        let state = AppState {
            doctor_panel_open: true,
            ..AppState::default()
        };
        assert!(matches!(
            state.active_modal(),
            Some(ActiveModal::DoctorPanel)
        ));

        let state = AppState {
            clean_confirm: Some(std::path::PathBuf::from("/tmp/proj")),
            ..AppState::default()
        };
        assert!(matches!(
            state.active_modal(),
            Some(ActiveModal::CleanConfirm(_))
        ));
    }

    /// Priority order matches the prior duplicated if/else chains: create
    /// wizard beats every other modal.
    #[test]
    fn create_wizard_wins_over_every_other_modal() {
        let state = AppState {
            create_wizard: Some(CreateWizard::new()),
            project_switcher_open: true,
            doctor_panel_open: true,
            clean_confirm: Some(std::path::PathBuf::from("/tmp/proj")),
            ..AppState::default()
        };
        assert!(matches!(
            state.active_modal(),
            Some(ActiveModal::CreateWizard(_))
        ));
    }

    /// Project switcher beats doctor panel, doctor panel beats clean-confirm
    /// — the same order the old chains checked them in.
    #[test]
    fn project_switcher_wins_over_doctor_panel_and_clean_confirm() {
        let state = AppState {
            project_switcher_open: true,
            doctor_panel_open: true,
            clean_confirm: Some(std::path::PathBuf::from("/tmp/proj")),
            ..AppState::default()
        };
        assert!(matches!(
            state.active_modal(),
            Some(ActiveModal::ProjectSwitcher)
        ));

        let state = AppState {
            doctor_panel_open: true,
            clean_confirm: Some(std::path::PathBuf::from("/tmp/proj")),
            ..AppState::default()
        };
        assert!(matches!(
            state.active_modal(),
            Some(ActiveModal::DoctorPanel)
        ));
    }
}
