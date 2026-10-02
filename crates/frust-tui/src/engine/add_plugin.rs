//! The "Add plugin" dialog state machine:
//! select a registry plugin → toggle its optional features → apply its
//! contributions to the open generated project off-thread → show a per-edit
//! report (or an error).
//!
//! Everything here is plain data + pure transitions — no filesystem, no
//! threads. The [`frust_drive::plugin::add_plugin`] run itself happens
//! off-thread in `crate::runner`, feeding its result back as a message
//! ([`super::Message::AddPluginSucceeded`] / [`super::Message::AddPluginFailed`]).
//! `crate::ui::views::add_plugin` renders it; the runner enacts the
//! [`Effect`](super::update::Effect)s the pure transition requests.
//!
//! Cloned in shape from [`super::create_wizard`] (the create-wizard modal
//! precedent): a linear step enum, sibling-gated selection cards, an off-thread
//! action step, and an error step with retry. No registry entry is
//! sibling-gated today — `clean-signals-frust` needs no
//! `requires_sibling` because `clean-signals` is a crates.io dependency
//! — but [`PluginEntry::sibling_gated`]/
//! [`AddPluginDialog::set_sibling_available`] stay in place as the generic
//! mechanism a future facade-tier plugin would reuse.

use std::path::PathBuf;

use frust_drive::plugin::known_plugins;

pub use frust_drive::plugin::AddReport;

/// Which step of the dialog is showing.
///
/// Linear: [`Select`](AddPluginStep::Select) →
/// [`Options`](AddPluginStep::Options) → [`Applying`](AddPluginStep::Applying)
/// (off-thread work in flight) → [`Report`](AddPluginStep::Report) on success;
/// a failure lands on [`Error`](AddPluginStep::Error), from which the user
/// retries or steps back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddPluginStep {
    /// The plugin selection cards, enumerated from
    /// [`frust_drive::plugin::known_plugins`].
    Select,
    /// The selected plugin's optional-feature checkboxes.
    Options,
    /// The off-thread `add_plugin` run is in flight (a spinner; no input).
    Applying,
    /// The apply succeeded; [`AddPluginDialog::report`] carries the per-edit
    /// line items.
    Report,
    /// The apply failed; [`AddPluginDialog::error`] carries the message.
    Error,
}

/// One selectable plugin card in the [`AddPluginStep::Select`] step, projected
/// from a [`frust_drive::plugin::PluginSpec`] registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginEntry {
    /// The registry id passed to `add_plugin` (`"secure-storage"`, …).
    pub id: String,
    /// A one-line human summary shown under the id.
    pub summary: String,
    /// Whether this card can currently be chosen. A sibling-gated card starts
    /// disabled pending the off-thread probe and is enabled once the sibling
    /// checkout is confirmed (see [`AddPluginDialog::set_sibling_available`]).
    pub enabled: bool,
    /// Why the card is disabled (rendered inline), or `None` when enabled.
    pub disabled_reason: Option<String>,
    /// Whether this card depends on a sibling checkout (a facade-tier plugin) —
    /// the probe toggles [`enabled`](PluginEntry::enabled) for these.
    pub sibling_gated: bool,
    /// The optional-feature toggles this plugin offers, seeded from the registry
    /// (all unselected); copied into the dialog's live feature list when the
    /// card is chosen.
    pub features: Vec<FeatureToggle>,
}

/// One optional-feature checkbox in the [`AddPluginStep::Options`] step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureToggle {
    /// The feature id passed to `add_plugin(.., features)`.
    pub id: String,
    /// A one-line human summary (the checkbox row lists exactly what it adds).
    pub summary: String,
    /// Whether the user has checked this feature.
    pub selected: bool,
}

/// What an [`AddPluginDialog::advance`] (Enter / the primary button) resolved
/// to — the [`super::update`] layer turns [`Apply`](AddPluginAdvance::Apply)
/// into the off-thread add effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddPluginAdvance {
    /// The step advanced within the dialog (no effect needed).
    Stepped,
    /// The user submitted the options step — apply `id` with `features`.
    Apply {
        /// The selected plugin id.
        id: String,
        /// The ids of the checked optional features.
        features: Vec<String>,
    },
    /// The dialog should close entirely (Enter on the report step).
    Done,
    /// Nothing happened (a disabled card, or a non-interactive step).
    Blocked,
}

/// The "Add plugin" dialog state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPluginDialog {
    /// Which step is showing.
    pub step: AddPluginStep,
    /// The generated project the contributions are applied to.
    pub project_root: PathBuf,
    /// The selectable plugin cards.
    pub entries: Vec<PluginEntry>,
    /// The highlighted selection card (`↑`/`↓` move it, wrapping); a disabled
    /// card can be highlighted (to read its reason) but not chosen.
    pub cursor: usize,
    /// The live optional-feature toggles for the chosen plugin (set when
    /// advancing from [`AddPluginStep::Select`]).
    pub features: Vec<FeatureToggle>,
    /// The highlighted feature row in the options step (clamped).
    pub feature_cursor: usize,
    /// The per-edit report shown on [`AddPluginStep::Report`].
    pub report: Option<AddReport>,
    /// The failure message shown on [`AddPluginStep::Error`].
    pub error: Option<String>,
}

impl AddPluginDialog {
    /// A fresh dialog over `project_root`, on the select step. Sibling-gated
    /// cards start disabled pending the off-thread probe (see
    /// [`set_sibling_available`](AddPluginDialog::set_sibling_available)).
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            step: AddPluginStep::Select,
            project_root,
            entries: build_entries(),
            cursor: 0,
            features: Vec::new(),
            feature_cursor: 0,
            report: None,
            error: None,
        }
    }

    /// The currently highlighted selection card.
    pub fn selected_entry(&self) -> Option<&PluginEntry> {
        self.entries.get(self.cursor)
    }

    /// Move the selection cursor by `delta` (wrapping). A no-op with no cards.
    pub fn select_move(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        let cur = self.cursor.min(self.entries.len() - 1) as isize;
        self.cursor = (cur + delta).rem_euclid(n) as usize;
    }

    /// Move the selection cursor to `index` (mouse click parity); ignored if
    /// out of range.
    pub fn select_at(&mut self, index: usize) {
        if index < self.entries.len() {
            self.cursor = index;
        }
    }

    /// Move the feature cursor by `delta` (clamped). A no-op with no features.
    pub fn feature_move(&mut self, delta: isize) {
        if self.features.is_empty() {
            return;
        }
        let n = self.features.len() as isize;
        let cur = self.feature_cursor.min(self.features.len() - 1) as isize;
        self.feature_cursor = (cur + delta).clamp(0, n - 1) as usize;
    }

    /// Toggle the feature under the cursor (`Space`). A no-op with no features.
    pub fn toggle_feature(&mut self) {
        if let Some(f) = self.features.get_mut(self.feature_cursor) {
            f.selected = !f.selected;
        }
    }

    /// Toggle a feature by index (mouse click parity); ignored out of range.
    pub fn toggle_feature_at(&mut self, index: usize) {
        if let Some(f) = self.features.get_mut(index) {
            f.selected = !f.selected;
            self.feature_cursor = index;
        }
    }

    /// The ids of the checked features (the `add_plugin` `features` argument).
    pub fn selected_feature_ids(&self) -> Vec<String> {
        self.features
            .iter()
            .filter(|f| f.selected)
            .map(|f| f.id.clone())
            .collect()
    }

    /// Advance (Enter / the primary button): validate the current step and
    /// either move forward, request the apply, close, or block. See
    /// [`AddPluginAdvance`].
    pub fn advance(&mut self) -> AddPluginAdvance {
        match self.step {
            AddPluginStep::Select => match self.selected_entry() {
                Some(entry) if entry.enabled => {
                    self.features = entry.features.clone();
                    self.feature_cursor = 0;
                    self.step = AddPluginStep::Options;
                    AddPluginAdvance::Stepped
                }
                _ => AddPluginAdvance::Blocked,
            },
            AddPluginStep::Options => {
                let Some(entry) = self.selected_entry() else {
                    return AddPluginAdvance::Blocked;
                };
                let id = entry.id.clone();
                let features = self.selected_feature_ids();
                self.error = None;
                self.step = AddPluginStep::Applying;
                AddPluginAdvance::Apply { id, features }
            }
            // Enter on the report closes the dialog.
            AddPluginStep::Report => AddPluginAdvance::Done,
            // Enter on the error step retries from the options step.
            AddPluginStep::Error => {
                self.error = None;
                self.step = AddPluginStep::Options;
                AddPluginAdvance::Stepped
            }
            // No input while the off-thread apply is running.
            AddPluginStep::Applying => AddPluginAdvance::Blocked,
        }
    }

    /// Step back (Esc / the Back button). Returns `true` when the dialog should
    /// close entirely (Esc on the first/report step); otherwise the step moves
    /// back one. A no-op (returns `false`) while an apply is in flight.
    pub fn back(&mut self) -> bool {
        match self.step {
            AddPluginStep::Select => true,
            AddPluginStep::Options => {
                self.step = AddPluginStep::Select;
                false
            }
            AddPluginStep::Report => true,
            AddPluginStep::Error => {
                self.error = None;
                self.step = AddPluginStep::Options;
                false
            }
            AddPluginStep::Applying => false,
        }
    }

    /// Enable/disable the sibling-gated card(s) once the off-thread probe
    /// resolves whether the required sibling checkout is present (shared with
    /// the create wizard's `CleanSignalsProbed` probe).
    pub fn set_sibling_available(&mut self, available: bool) {
        for entry in &mut self.entries {
            if entry.sibling_gated {
                entry.enabled = available;
                entry.disabled_reason = (!available).then(|| SIBLING_ABSENT.to_string());
            }
        }
    }

    /// Move to the report step with a successful per-edit `report`.
    pub fn succeed(&mut self, report: AddReport) {
        self.report = Some(report);
        self.error = None;
        self.step = AddPluginStep::Report;
    }

    /// Move to the error step with `message` (an apply failure).
    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.step = AddPluginStep::Error;
    }
}

/// The disabled-reason shown on a sibling-gated card when the sibling checkout
/// is absent (mirrors the create wizard's clean-signals gating).
const SIBLING_ABSENT: &str = "needs a sibling checkout (dev-machine only)";

/// The pending disabled-reason on a sibling-gated card before the off-thread
/// probe resolves.
const SIBLING_PENDING: &str = "checking for the required sibling checkout…";

/// Build the selection cards from the static plugin registry
/// ([`known_plugins`]). A card that requires a sibling checkout starts disabled
/// pending the probe; every other card is immediately selectable.
fn build_entries() -> Vec<PluginEntry> {
    known_plugins()
        .into_iter()
        .map(|spec| {
            let sibling_gated = spec.requires_sibling.is_some();
            PluginEntry {
                id: spec.id.to_string(),
                summary: spec.summary.to_string(),
                enabled: !sibling_gated,
                disabled_reason: sibling_gated.then(|| SIBLING_PENDING.to_string()),
                sibling_gated,
                features: spec
                    .optional_features
                    .iter()
                    .map(|f| FeatureToggle {
                        id: f.id.to_string(),
                        summary: f.summary.to_string(),
                        selected: false,
                    })
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::plugin::{AddItem, AddOutcome};

    fn dialog() -> AddPluginDialog {
        AddPluginDialog::new(PathBuf::from("/tmp/proj"))
    }

    #[test]
    fn starts_on_select_enumerating_the_registry() {
        let d = dialog();
        assert_eq!(d.step, AddPluginStep::Select);
        // One card per known plugin, in registry order.
        assert_eq!(d.entries.len(), known_plugins().len());
        assert_eq!(d.entries[0].id, "shared-preferences");
    }

    #[test]
    fn no_registry_entry_is_currently_sibling_gated() {
        // clean-signals-frust needs no `requires_sibling` (clean-signals is
        // a crates.io dependency), so every card starts enabled.
        let d = dialog();
        assert!(d.entries.iter().all(|e| !e.sibling_gated && e.enabled));
    }

    /// The generic sibling-gating mechanism ([`PluginEntry::sibling_gated`] /
    /// [`AddPluginDialog::set_sibling_available`]) has no live registry user
    /// today — exercise it directly against a synthetic gated entry rather
    /// than through a real plugin id.
    #[test]
    fn set_sibling_available_toggles_a_gated_entrys_enabled_state() {
        let mut d = dialog();
        d.entries.push(PluginEntry {
            id: "future-facade-plugin".to_string(),
            summary: "stand-in for a future sibling-gated plugin".to_string(),
            enabled: false,
            disabled_reason: Some(SIBLING_PENDING.to_string()),
            sibling_gated: true,
            features: Vec::new(),
        });
        let idx = d.entries.len() - 1;

        d.set_sibling_available(true);
        assert!(d.entries[idx].enabled);
        assert!(d.entries[idx].disabled_reason.is_none());

        d.set_sibling_available(false);
        assert!(!d.entries[idx].enabled);
        assert!(d.entries[idx].disabled_reason.is_some());
    }

    #[test]
    fn select_cursor_wraps_and_can_land_on_disabled_cards() {
        let mut d = dialog();
        let n = d.entries.len();
        d.select_move(-1);
        assert_eq!(d.cursor, n - 1);
        d.select_move(1);
        assert_eq!(d.cursor, 0);
        d.select_at(n - 1);
        assert_eq!(d.cursor, n - 1);
    }

    #[test]
    fn advancing_a_disabled_card_is_blocked() {
        let mut d = dialog();
        // No registry entry starts disabled today (see
        // `no_registry_entry_is_currently_sibling_gated`), so exercise the
        // disabled-blocks-advance contract against a synthetic gated entry.
        d.entries.push(PluginEntry {
            id: "future-facade-plugin".to_string(),
            summary: "stand-in for a future sibling-gated plugin".to_string(),
            enabled: false,
            disabled_reason: Some(SIBLING_PENDING.to_string()),
            sibling_gated: true,
            features: Vec::new(),
        });
        let idx = d.entries.len() - 1;
        d.cursor = idx;
        d.set_sibling_available(false);
        assert_eq!(d.advance(), AddPluginAdvance::Blocked);
        assert_eq!(d.step, AddPluginStep::Select);
    }

    #[test]
    fn advancing_an_enabled_card_seeds_its_feature_toggles() {
        let mut d = dialog();
        // secure-storage has an optional biometric-gate feature.
        let idx = d
            .entries
            .iter()
            .position(|e| e.id == "secure-storage")
            .unwrap();
        d.cursor = idx;
        assert_eq!(d.advance(), AddPluginAdvance::Stepped);
        assert_eq!(d.step, AddPluginStep::Options);
        assert_eq!(d.features.len(), 1);
        assert_eq!(d.features[0].id, "biometric-gate");
        assert!(!d.features[0].selected);
    }

    #[test]
    fn toggling_a_feature_carries_it_into_the_apply_effect() {
        let mut d = dialog();
        let idx = d
            .entries
            .iter()
            .position(|e| e.id == "secure-storage")
            .unwrap();
        d.cursor = idx;
        d.advance(); // Select -> Options
        // No feature checked yet → apply with an empty feature list.
        let mut probe = d.clone();
        assert_eq!(
            probe.advance(),
            AddPluginAdvance::Apply {
                id: "secure-storage".to_string(),
                features: Vec::new(),
            }
        );
        // Check the biometric gate, then apply carries it.
        d.toggle_feature();
        assert!(d.features[0].selected);
        assert_eq!(
            d.advance(),
            AddPluginAdvance::Apply {
                id: "secure-storage".to_string(),
                features: vec!["biometric-gate".to_string()],
            }
        );
        assert_eq!(d.step, AddPluginStep::Applying);
    }

    #[test]
    fn toggle_feature_at_moves_the_cursor_and_flips_it() {
        let mut d = dialog();
        let idx = d
            .entries
            .iter()
            .position(|e| e.id == "secure-storage")
            .unwrap();
        d.cursor = idx;
        d.advance();
        d.toggle_feature_at(0);
        assert!(d.features[0].selected);
        assert_eq!(d.feature_cursor, 0);
    }

    #[test]
    fn a_plugin_with_no_features_still_applies() {
        let mut d = dialog();
        // shared-preferences has no optional features.
        let idx = d
            .entries
            .iter()
            .position(|e| e.id == "shared-preferences")
            .unwrap();
        d.cursor = idx;
        assert_eq!(d.advance(), AddPluginAdvance::Stepped);
        assert!(d.features.is_empty());
        assert_eq!(
            d.advance(),
            AddPluginAdvance::Apply {
                id: "shared-preferences".to_string(),
                features: Vec::new(),
            }
        );
    }

    #[test]
    fn back_steps_and_closes() {
        let mut d = dialog();
        assert!(d.back(), "Esc on the select step closes the dialog");

        let mut d = dialog();
        d.step = AddPluginStep::Options;
        assert!(!d.back());
        assert_eq!(d.step, AddPluginStep::Select);
    }

    #[test]
    fn success_lands_on_the_report_step() {
        let mut d = dialog();
        d.step = AddPluginStep::Applying;
        let report = AddReport {
            plugin_id: "secure-storage".to_string(),
            items: vec![
                AddItem {
                    description: "Cargo.toml dependency `frust-secure-storage`".to_string(),
                    outcome: AddOutcome::Applied,
                },
                AddItem {
                    description: "AndroidManifest.xml permission `x`".to_string(),
                    outcome: AddOutcome::AlreadyPresent,
                },
            ],
        };
        d.succeed(report.clone());
        assert_eq!(d.step, AddPluginStep::Report);
        assert_eq!(d.report.as_ref().unwrap().counts(), (1, 1));
        // Enter on the report closes the dialog.
        assert_eq!(d.advance(), AddPluginAdvance::Done);
    }

    #[test]
    fn failure_lands_on_the_error_step_and_retries() {
        let mut d = dialog();
        d.step = AddPluginStep::Applying;
        d.fail("no `frust` dependency".to_string());
        assert_eq!(d.step, AddPluginStep::Error);
        assert_eq!(d.error.as_deref(), Some("no `frust` dependency"));
        // Enter retries from the options step.
        assert_eq!(d.advance(), AddPluginAdvance::Stepped);
        assert_eq!(d.step, AddPluginStep::Options);
        assert!(d.error.is_none());
    }
}
