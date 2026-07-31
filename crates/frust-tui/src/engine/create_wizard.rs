//! The create-project wizard state machine: name → directory →
//! architecture → off-thread scaffold → open-in-place.
//!
//! Everything here is plain data + pure transitions — no filesystem, no
//! threads. The `frust_drive::scaffold::generate` run itself happens
//! off-thread in `crate::runner`, feeding its result back as a message
//! ([`super::Message::ScaffoldSucceeded`] / [`super::Message::ScaffoldFailed`]).
//! `crate::ui` renders it; the runner enacts the
//! [`Effect`](super::update::Effect)s the pure transition requests.
//!
//! No arch card is sibling-gated today — `clean-signals` is git+rev-pinned to
//! its public repo (`docs/DEVELOPMENT.md`'s Version-Pin Policy), so no
//! `../clean-signals-rs` sibling checkout is required — but
//! [`ArchCard::sibling_gated`]/[`CreateWizard::set_clean_signals_available`]
//! stay in place as the generic mechanism a future sibling-dependent arch
//! would reuse.

use frust_drive::scaffold::{KNOWN_ARCHES, validate_project_name};

/// Which step of the wizard is showing.
///
/// Linear top-to-bottom: [`Name`](WizardStep::Name) →
/// [`Directory`](WizardStep::Directory) → [`Arch`](WizardStep::Arch) →
/// [`Scaffolding`](WizardStep::Scaffolding) (off-thread work in flight) →
/// success closes the wizard and opens the project; a scaffold failure lands
/// on [`Error`](WizardStep::Error), from which the user retries or steps back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// The project-name field, validated live against the drive's crate-name
    /// rules ([`validate_project_name`]).
    Name,
    /// The target-directory field (defaults to mirroring the name).
    Directory,
    /// The architecture cards, enumerated from [`KNOWN_ARCHES`].
    Arch,
    /// The off-thread `generate` run is in flight (a spinner; no input).
    Scaffolding,
    /// The scaffold failed; [`CreateWizard::error`] carries the message.
    Error,
}

/// One architecture card in the [`WizardStep::Arch`] step: the default
/// template plus one card per [`KNOWN_ARCHES`] tag (new tags appear here with
/// no wizard change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchCard {
    /// The `arch` argument passed to `frust_drive::scaffold::generate` when
    /// this card is chosen — `None` for the default template.
    pub tag: Option<String>,
    /// The human-facing card title (`Default`, `Clean Signals`).
    pub label: String,
    /// A one-line description shown under the label.
    pub description: String,
    /// Whether this card can currently be chosen. A sibling-gated card starts
    /// disabled and is enabled once the off-thread probe confirms the sibling
    /// checkout (see [`CreateWizard::set_clean_signals_available`]). No card
    /// is sibling-gated today (see this module's doc comment).
    pub enabled: bool,
    /// Why the card is disabled (rendered inline), or `None` when enabled.
    pub disabled_reason: Option<String>,
    /// Whether this card depends on a sibling checkout — the probe toggles
    /// [`enabled`](ArchCard::enabled) for these. No arch needs one today; a
    /// future arch that does adds its own gating here.
    pub sibling_gated: bool,
}

/// What an [`CreateWizard::advance`] (Enter / the Next-or-Create button)
/// resolved to — the [`super::update`] layer turns [`Scaffold`](WizardAdvance::Scaffold)
/// into the off-thread scaffold effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardAdvance {
    /// The step advanced within the wizard (no effect needed).
    Stepped,
    /// The user submitted the arch step — scaffold with this `arch` tag.
    Scaffold {
        /// The selected card's `arch` argument (`None` = default template).
        arch: Option<String>,
    },
    /// Nothing happened (an invalid field, a disabled card, or a
    /// non-interactive step).
    Blocked,
}

/// The create-project wizard state.
#[derive(Debug, Clone)]
pub struct CreateWizard {
    /// Which step is showing.
    pub step: WizardStep,
    /// The project name being typed.
    pub name: String,
    /// The live name-validation error, or `None` when the name is empty or
    /// valid.
    pub name_error: Option<String>,
    /// The target directory (relative paths resolve against the process cwd
    /// when the runner enacts the scaffold).
    pub directory: String,
    /// While `false`, [`directory`](CreateWizard::directory) mirrors
    /// [`name`](CreateWizard::name); the first manual edit to the directory
    /// field latches this `true` and stops the mirror.
    directory_edited: bool,
    /// The architecture cards (default + one per [`KNOWN_ARCHES`] tag).
    pub arches: Vec<ArchCard>,
    /// The highlighted arch card (`←`/`→`/`↑`/`↓` move it, wrapping); a
    /// disabled card can be highlighted (to read its explanation) but not
    /// chosen.
    pub arch_cursor: usize,
    /// A scaffold failure message, shown on [`WizardStep::Error`].
    pub error: Option<String>,
}

impl Default for CreateWizard {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateWizard {
    /// A fresh wizard on the name step. Sibling-gated arch cards start
    /// disabled pending the off-thread probe (see
    /// [`set_clean_signals_available`](CreateWizard::set_clean_signals_available)).
    pub fn new() -> Self {
        Self {
            step: WizardStep::Name,
            name: String::new(),
            name_error: None,
            directory: String::new(),
            directory_edited: false,
            arches: build_cards(),
            arch_cursor: 0,
            error: None,
        }
    }

    /// Feed a character into the focused text field (name/directory); a no-op
    /// on any non-text step. Typing the name re-validates it live and, until
    /// the directory has been edited, mirrors it into the directory field.
    pub fn input_char(&mut self, c: char) {
        match self.step {
            WizardStep::Name => {
                self.name.push(c);
                self.revalidate_name();
                self.sync_directory();
            }
            WizardStep::Directory => {
                self.directory.push(c);
                self.directory_edited = true;
            }
            _ => {}
        }
    }

    /// Delete the last character of the focused text field; a no-op elsewhere.
    pub fn backspace(&mut self) {
        match self.step {
            WizardStep::Name => {
                self.name.pop();
                self.revalidate_name();
                self.sync_directory();
            }
            WizardStep::Directory => {
                self.directory.pop();
                self.directory_edited = true;
            }
            _ => {}
        }
    }

    /// Advance (Enter / the Next-or-Create action): validate the current step
    /// and either move forward, request a scaffold, or block. See
    /// [`WizardAdvance`].
    pub fn advance(&mut self) -> WizardAdvance {
        match self.step {
            WizardStep::Name => {
                self.revalidate_name();
                if self.name_is_valid() {
                    self.step = WizardStep::Directory;
                    WizardAdvance::Stepped
                } else {
                    WizardAdvance::Blocked
                }
            }
            WizardStep::Directory => {
                if self.directory.trim().is_empty() {
                    WizardAdvance::Blocked
                } else {
                    self.step = WizardStep::Arch;
                    WizardAdvance::Stepped
                }
            }
            WizardStep::Arch => match self.selected_card() {
                Some(card) if card.enabled => {
                    let arch = card.tag.clone();
                    self.error = None;
                    self.step = WizardStep::Scaffolding;
                    WizardAdvance::Scaffold { arch }
                }
                _ => WizardAdvance::Blocked,
            },
            // Enter on the error step retries the scaffold from the arch step.
            WizardStep::Error => {
                self.error = None;
                self.step = WizardStep::Arch;
                WizardAdvance::Stepped
            }
            // No input while the off-thread scaffold is running.
            WizardStep::Scaffolding => WizardAdvance::Blocked,
        }
    }

    /// Step back (Esc / the Back button). Returns `true` when the wizard
    /// should close entirely (Esc on the first step); otherwise the step moves
    /// back one. A no-op (returns `false`) while a scaffold is in flight.
    pub fn back(&mut self) -> bool {
        match self.step {
            WizardStep::Name => true,
            WizardStep::Directory => {
                self.step = WizardStep::Name;
                false
            }
            WizardStep::Arch => {
                self.step = WizardStep::Directory;
                false
            }
            WizardStep::Error => {
                self.error = None;
                self.step = WizardStep::Arch;
                false
            }
            WizardStep::Scaffolding => false,
        }
    }

    /// Move the arch-card cursor by `delta` (wrapping). A no-op with no cards.
    pub fn arch_move(&mut self, delta: isize) {
        if self.arches.is_empty() {
            return;
        }
        let n = self.arches.len() as isize;
        let cur = self.arch_cursor.min(self.arches.len() - 1) as isize;
        self.arch_cursor = (cur + delta).rem_euclid(n) as usize;
    }

    /// Move the arch-card cursor to `index` (mouse click parity); ignored if
    /// out of range.
    pub fn select_arch_at(&mut self, index: usize) {
        if index < self.arches.len() {
            self.arch_cursor = index;
        }
    }

    /// Enable/disable the sibling-gated arch card(s) once the off-thread probe
    /// resolves whether a required sibling checkout is present. Currently a
    /// no-op — no arch card is sibling-gated (this module's doc comment) —
    /// kept for a future sibling-dependent arch to reuse.
    pub fn set_clean_signals_available(&mut self, available: bool) {
        for card in &mut self.arches {
            if card.sibling_gated {
                card.enabled = available;
                card.disabled_reason = (!available).then(|| CLEAN_SIGNALS_ABSENT.to_string());
            }
        }
    }

    /// Move to the error step with `message` (a scaffold failure).
    pub fn fail(&mut self, message: String) {
        self.error = Some(message);
        self.step = WizardStep::Error;
    }

    /// The currently highlighted arch card.
    pub fn selected_card(&self) -> Option<&ArchCard> {
        self.arches.get(self.arch_cursor)
    }

    /// Whether the typed name is a usable crate name (non-empty and passing
    /// live validation) — the [`WizardStep::Name`] advance precondition.
    pub fn name_is_valid(&self) -> bool {
        !self.name.is_empty() && self.name_error.is_none()
    }

    fn revalidate_name(&mut self) {
        self.name_error = if self.name.is_empty() {
            None
        } else {
            validate_project_name(&self.name)
                .err()
                .map(|e| e.to_string())
        };
    }

    fn sync_directory(&mut self) {
        if !self.directory_edited {
            self.directory = self.name.clone();
        }
    }
}

/// The disabled-reason shown on a sibling-gated card when the sibling checkout
/// is absent (mirrors the CLI `--arch` flag's dev-machine-only caveat).
const CLEAN_SIGNALS_ABSENT: &str =
    "needs the ../clean-signals-rs sibling checkout (dev-machine only)";

/// The pending disabled-reason on a sibling-gated card before the off-thread
/// probe resolves.
const CLEAN_SIGNALS_PENDING: &str = "checking for the ../clean-signals-rs sibling…";

/// Build the arch cards: the default template, then one card per
/// [`KNOWN_ARCHES`] tag. A sibling-gated card would start disabled pending
/// the probe — no known tag needs one today (see this module's doc comment).
fn build_cards() -> Vec<ArchCard> {
    let mut cards = vec![ArchCard {
        tag: None,
        label: "Default".to_string(),
        description: "The standard notes-app template.".to_string(),
        enabled: true,
        disabled_reason: None,
        sibling_gated: false,
    }];
    for &tag in KNOWN_ARCHES {
        let sibling_gated = false;
        cards.push(ArchCard {
            tag: Some(tag.to_string()),
            label: humanize(tag),
            description: arch_description(tag),
            enabled: !sibling_gated,
            disabled_reason: sibling_gated.then(|| CLEAN_SIGNALS_PENDING.to_string()),
            sibling_gated,
        });
    }
    cards
}

/// A human-facing card title from a hyphen/underscore arch tag
/// (`clean-signals` → `Clean Signals`).
fn humanize(tag: &str) -> String {
    tag.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A one-line description per known arch tag; an unrecognized future tag gets
/// a generic line (still enumerated with no wizard change).
fn arch_description(tag: &str) -> String {
    match tag {
        "clean-signals" => {
            "Controller + use-case + async_view via clean-signals-frust.".to_string()
        }
        _ => "An architecture variant.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_on_name_with_default_arch_selected() {
        let w = CreateWizard::new();
        assert_eq!(w.step, WizardStep::Name);
        assert_eq!(w.arch_cursor, 0);
        assert_eq!(w.selected_card().unwrap().tag, None);
    }

    #[test]
    fn cards_enumerate_default_plus_known_arches() {
        let w = CreateWizard::new();
        // Default + every KNOWN_ARCHES tag.
        assert_eq!(w.arches.len(), 1 + KNOWN_ARCHES.len());
        assert_eq!(w.arches[0].tag, None);
        let clean = w
            .arches
            .iter()
            .find(|c| c.tag.as_deref() == Some("clean-signals"))
            .expect("clean-signals card enumerated from KNOWN_ARCHES");
        assert_eq!(clean.label, "Clean Signals");
        // clean-signals is git+rev-pinned to its public repo (task 05), so
        // this card needs no sibling checkout and starts enabled like every
        // other card.
        assert!(!clean.sibling_gated);
        assert!(clean.enabled);
    }

    /// The generic sibling-gating mechanism ([`ArchCard::sibling_gated`] /
    /// [`CreateWizard::set_clean_signals_available`]) has no live user today
    /// — exercise it directly against a synthetic gated card rather than
    /// through `clean-signals`, which is no longer gated.
    #[test]
    fn set_clean_signals_available_toggles_a_gated_cards_enabled_state() {
        let mut w = CreateWizard::new();
        w.arches.push(ArchCard {
            tag: Some("future-sibling-arch".to_string()),
            label: "Future Sibling Arch".to_string(),
            description: "stand-in for a future sibling-gated arch".to_string(),
            enabled: false,
            disabled_reason: Some(CLEAN_SIGNALS_PENDING.to_string()),
            sibling_gated: true,
        });
        let idx = w.arches.len() - 1;

        w.set_clean_signals_available(false);
        assert!(!w.arches[idx].enabled);
        assert!(w.arches[idx].disabled_reason.is_some());

        w.set_clean_signals_available(true);
        assert!(w.arches[idx].enabled);
        assert!(w.arches[idx].disabled_reason.is_none());
    }

    #[test]
    fn name_validates_live() {
        let mut w = CreateWizard::new();
        for c in "my-app".chars() {
            w.input_char(c);
        }
        // A hyphen is invalid for a crate name.
        assert!(w.name_error.is_some());
        assert!(!w.name_is_valid());
        // Advancing is blocked on an invalid name.
        assert_eq!(w.advance(), WizardAdvance::Blocked);
        assert_eq!(w.step, WizardStep::Name);
    }

    #[test]
    fn directory_mirrors_name_until_edited() {
        let mut w = CreateWizard::new();
        for c in "my_app".chars() {
            w.input_char(c);
        }
        assert_eq!(w.directory, "my_app");
        // Advance to the directory step and edit it — the mirror latches off.
        assert_eq!(w.advance(), WizardAdvance::Stepped);
        assert_eq!(w.step, WizardStep::Directory);
        w.input_char('2');
        assert_eq!(w.directory, "my_app2");
        // Going back and editing the name no longer overwrites the directory.
        assert!(!w.back());
        assert_eq!(w.step, WizardStep::Name);
        w.input_char('x');
        assert_eq!(w.name, "my_appx");
        assert_eq!(w.directory, "my_app2");
    }

    #[test]
    fn full_advance_to_scaffold_carries_the_selected_arch() {
        let mut w = CreateWizard::new();
        for c in "my_app".chars() {
            w.input_char(c);
        }
        assert_eq!(w.advance(), WizardAdvance::Stepped); // Name -> Directory
        assert_eq!(w.advance(), WizardAdvance::Stepped); // Directory -> Arch
        assert_eq!(w.step, WizardStep::Arch);
        // Default card (arch None) is enabled and scaffolds.
        assert_eq!(w.advance(), WizardAdvance::Scaffold { arch: None });
        assert_eq!(w.step, WizardStep::Scaffolding);
    }

    #[test]
    fn a_disabled_card_blocks_scaffold() {
        let mut w = CreateWizard::new();
        // No arch card is disabled by default today (see
        // `cards_enumerate_default_plus_known_arches`), so exercise the
        // disabled-blocks-advance contract against a synthetic gated card.
        w.arches.push(ArchCard {
            tag: Some("future-sibling-arch".to_string()),
            label: "Future Sibling Arch".to_string(),
            description: "stand-in for a future sibling-gated arch".to_string(),
            enabled: false,
            disabled_reason: Some(CLEAN_SIGNALS_PENDING.to_string()),
            sibling_gated: true,
        });
        let idx = w.arches.len() - 1;
        w.step = WizardStep::Arch;
        w.arch_cursor = idx;
        w.set_clean_signals_available(false);
        // Highlighting the disabled card is fine; choosing it is blocked.
        assert_eq!(w.advance(), WizardAdvance::Blocked);
        assert_eq!(w.step, WizardStep::Arch);
        // Once the sibling probe enables it, it scaffolds with the tag.
        w.set_clean_signals_available(true);
        assert_eq!(
            w.advance(),
            WizardAdvance::Scaffold {
                arch: Some("future-sibling-arch".to_string())
            }
        );
    }

    #[test]
    fn arch_cursor_wraps_and_can_land_on_disabled_cards() {
        let mut w = CreateWizard::new();
        w.step = WizardStep::Arch;
        let n = w.arches.len();
        w.arch_move(-1);
        assert_eq!(w.arch_cursor, n - 1);
        w.arch_move(1);
        assert_eq!(w.arch_cursor, 0);
    }

    #[test]
    fn back_from_name_closes_and_error_retries() {
        let mut w = CreateWizard::new();
        assert!(w.back(), "Esc on the first step closes the wizard");

        let mut w = CreateWizard::new();
        w.fail("boom".to_string());
        assert_eq!(w.step, WizardStep::Error);
        assert_eq!(w.error.as_deref(), Some("boom"));
        // Enter retries from the arch step; error is cleared.
        assert_eq!(w.advance(), WizardAdvance::Stepped);
        assert_eq!(w.step, WizardStep::Arch);
        assert!(w.error.is_none());
    }
}
