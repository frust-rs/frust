//! The toolchain bootstrap wizard: the fresh-machine
//! flow that walks a machine missing `cargo-ndk` / rustup targets / a JDK to a
//! building-Frust-apps state, modeled on fdemon 0.6.3's InstallWizard (a
//! PATTERN source only — BSL-1.1, no verbatim copies).
//!
//! Everything here is plain data + pure transitions over the component-level
//! report the drive side produces
//! ([`frust_drive::doctor::build_report`]): the wizard holds a snapshot of that
//! [`DoctorReport`] plus the fdemon-style collapsed/expanded tree cursor, and
//! projects it into a left step tree + a right detail pane. The two pieces of
//! real work — running the off-thread report and running a guided fix command
//! as a supervised session — happen in `crate::runner`, feeding results back as
//! messages ([`super::Message::BootstrapReport`] and the session events a fix
//! command streams). `crate::ui::views::bootstrap` renders it; the runner
//! enacts the [`Effect`](super::update::Effect)s the pure transition requests.

use frust_drive::doctor::{Component, ComponentStatus, DoctorReport, FixCommand};

/// The report area name whose `Missing` gates handback — a `Missing` core
/// (Rust toolchain / cargo) blocks, every platform area's gap stays a
/// non-blocking `Partial` (mirrors `frust_drive::doctor::report`'s
/// own `CORE_AREA`, which isn't re-exported, so it's named here with this
/// note).
pub const CORE_AREA: &str = "Prerequisites";

/// The cached component-level report the titlebar chip's rollup and the wizard
/// both read, plus whether a preflight is in flight and whether the
/// fresh-machine auto-open has already fired this launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootstrapState {
    /// The last full report (`None` before the first startup preflight
    /// completes — the chip shows "checking…" until then).
    pub report: Option<DoctorReport>,
    /// Whether an off-thread report run is currently in flight.
    pub refreshing: bool,
    /// Whether the fresh-machine auto-open (on a `Missing` core rollup) has
    /// already been considered this launch — it fires at most once so a user
    /// who closes it isn't re-nagged on every re-preflight.
    pub auto_shown: bool,
}

impl BootstrapState {
    /// The titlebar-chip rollup from the cached report, or `None` before the
    /// first report arrives.
    pub fn rollup(&self) -> Option<ComponentStatus> {
        self.report.as_ref().map(DoctorReport::rollup)
    }
}

/// One node in the wizard's left step tree (fdemon's collapsed/expanded
/// projection). The tree is: one leaf per core (`Prerequisites`) component, a
/// `Platforms` header collapsing the platform areas, each platform area (when
/// expanded), and a `Doctor rollup` summary leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapNode {
    /// A `Prerequisites` component leaf (`area`/`component` indices into the
    /// report).
    Core { area: usize, component: usize },
    /// The expandable `Platforms` header.
    PlatformsHeader,
    /// A platform area leaf (Android / iOS / Desktop), shown only when the
    /// header is expanded (`area` index into the report).
    Platform { area: usize },
    /// The whole-report rollup summary leaf.
    Rollup,
}

/// The bootstrap wizard's state: a snapshot of the report plus the tree cursor
/// (all pure — see the module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapWizard {
    /// The report snapshot the tree projects (refreshed in place when a
    /// re-preflight lands while the wizard is open).
    pub report: DoctorReport,
    /// Whether the `Platforms` header is expanded (fdemon's collapsed/expanded
    /// projection). Starts expanded so a fresh machine sees its platform gaps.
    pub platforms_expanded: bool,
    /// The selected step-tree row (index into [`Self::nodes`], clamped).
    pub cursor: usize,
    /// The selected fix-command row in the detail pane (index into
    /// [`Self::current_fixes`], clamped).
    pub fix_cursor: usize,
}

impl BootstrapWizard {
    /// Open a wizard over `report`, cursor on the first step, platforms
    /// expanded.
    pub fn from_report(report: DoctorReport) -> Self {
        Self {
            report,
            platforms_expanded: true,
            cursor: 0,
            fix_cursor: 0,
        }
    }

    /// Replace the report snapshot in place (a re-preflight landed while the
    /// wizard is open), clamping both cursors into the new projection.
    pub fn set_report(&mut self, report: DoctorReport) {
        self.report = report;
        self.clamp();
    }

    /// The whole-report rollup — the handback-gating status: a
    /// `Missing` core blocks, platform gaps stay `Partial`.
    pub fn rollup(&self) -> ComponentStatus {
        self.report.rollup()
    }

    /// Whether the core (Rust toolchain) gates handback — `true` only when the
    /// core area is `Missing` (fdemon's non-blocking-platform-gap model).
    pub fn core_gates(&self) -> bool {
        self.core_area_idx().map(|i| self.report.areas[i].status())
            == Some(ComponentStatus::Missing)
    }

    /// The index of the core (`Prerequisites`) area, if present.
    fn core_area_idx(&self) -> Option<usize> {
        self.report.areas.iter().position(|a| a.name == CORE_AREA)
    }

    /// The indices of the platform (non-core) areas, in report order.
    fn platform_area_indices(&self) -> Vec<usize> {
        self.report
            .areas
            .iter()
            .enumerate()
            .filter(|(_, a)| a.name != CORE_AREA)
            .map(|(i, _)| i)
            .collect()
    }

    /// The visible step-tree nodes, in render order — the collapsed/expanded
    /// projection (platform leaves appear only while the header is expanded).
    pub fn nodes(&self) -> Vec<BootstrapNode> {
        let mut nodes = Vec::new();
        if let Some(core) = self.core_area_idx() {
            for c in 0..self.report.areas[core].components.len() {
                nodes.push(BootstrapNode::Core {
                    area: core,
                    component: c,
                });
            }
        }
        nodes.push(BootstrapNode::PlatformsHeader);
        if self.platforms_expanded {
            for area in self.platform_area_indices() {
                nodes.push(BootstrapNode::Platform { area });
            }
        }
        nodes.push(BootstrapNode::Rollup);
        nodes
    }

    /// The currently-selected node (cursor clamped into the projection).
    pub fn current_node(&self) -> BootstrapNode {
        let nodes = self.nodes();
        nodes[self.cursor.min(nodes.len() - 1)]
    }

    /// The status glyph key for a node. A `Core` leaf shows its component's own
    /// (possibly `Missing`) status; a `Platform` leaf and the `Platforms`
    /// header cap at `Partial` (a platform gap is non-blocking — the missing
    /// component rows in the detail pane still render `Missing`, but the
    /// platform *rollup* only ever degrades to `Partial`, fdemon's model), and
    /// `Rollup` shows the whole-report rollup (which caps the same way).
    pub fn node_status(&self, node: BootstrapNode) -> ComponentStatus {
        match node {
            BootstrapNode::Core { area, component } => {
                self.report.areas[area].components[component].status
            }
            BootstrapNode::PlatformsHeader => cap_platform(worst(
                self.platform_area_indices()
                    .into_iter()
                    .map(|i| self.report.areas[i].status()),
            )),
            BootstrapNode::Platform { area } => cap_platform(self.report.areas[area].status()),
            BootstrapNode::Rollup => self.rollup(),
        }
    }

    /// The non-blocking (capped-at-`Partial`) status a platform area/header
    /// shows — the same value [`Self::node_status`] uses, exposed for the
    /// detail-pane header.
    pub fn platform_display_status(&self, area: usize) -> ComponentStatus {
        cap_platform(self.report.areas[area].status())
    }

    /// The components the detail (right) pane lists for the current node: the
    /// single component for a `Core` leaf, every component of the area for a
    /// `Platform` leaf, and none for the header/rollup summaries.
    pub fn detail_components(&self) -> Vec<&Component> {
        match self.current_node() {
            BootstrapNode::Core { area, component } => {
                vec![&self.report.areas[area].components[component]]
            }
            BootstrapNode::Platform { area } => self.report.areas[area].components.iter().collect(),
            BootstrapNode::PlatformsHeader | BootstrapNode::Rollup => Vec::new(),
        }
    }

    /// The fix commands the detail pane offers for the current node, each
    /// paired with the component it fixes — every fix across the node's
    /// component(s), in order. The `fix_cursor` indexes this list.
    pub fn current_fixes(&self) -> Vec<(&Component, &FixCommand)> {
        self.detail_components()
            .into_iter()
            .flat_map(|c| c.fix_commands.iter().map(move |f| (c, f)))
            .collect()
    }

    /// The currently-selected fix command (clamped), if the current node has
    /// any.
    pub fn selected_fix(&self) -> Option<&FixCommand> {
        let fixes = self.current_fixes();
        if fixes.is_empty() {
            return None;
        }
        Some(fixes[self.fix_cursor.min(fixes.len() - 1)].1)
    }

    /// The selected fix, but only when it's auto-runnable as a supervised
    /// session (a privileged/system install stays guidance-only — `None`
    /// here).
    pub fn runnable_selected_fix(&self) -> Option<&FixCommand> {
        self.selected_fix().filter(|f| f.auto_runnable)
    }

    /// Move the step cursor by `delta` (clamped), resetting the fix cursor when
    /// the selected step changes. Returns whether anything moved.
    pub fn nav(&mut self, delta: isize) -> bool {
        let n = self.nodes().len() as isize;
        let cur = self.cursor.min(self.nodes().len() - 1) as isize;
        let next = (cur + delta).clamp(0, n - 1) as usize;
        if next == self.cursor {
            return false;
        }
        self.cursor = next;
        self.fix_cursor = 0;
        true
    }

    /// Move the fix cursor by `delta` (clamped to the current node's fix
    /// list). Returns whether anything moved.
    pub fn nav_fix(&mut self, delta: isize) -> bool {
        let n = self.current_fixes().len();
        if n == 0 {
            return false;
        }
        let cur = self.fix_cursor.min(n - 1) as isize;
        let next = (cur + delta).clamp(0, n as isize - 1) as usize;
        if next == self.fix_cursor {
            return false;
        }
        self.fix_cursor = next;
        true
    }

    /// Toggle the `Platforms` header expansion (only meaningful while the
    /// header is the selected node — the caller gates this). Clamps the cursor
    /// afterward since collapsing removes rows.
    pub fn toggle_expand(&mut self) {
        self.platforms_expanded = !self.platforms_expanded;
        self.clamp();
    }

    /// Move the step cursor to `index` (mouse click parity), resetting the fix
    /// cursor. Ignored when out of range.
    pub fn select_step(&mut self, index: usize) {
        if index < self.nodes().len() && index != self.cursor {
            self.cursor = index;
            self.fix_cursor = 0;
        }
    }

    /// Move the fix cursor to `index` (mouse click parity). Ignored out of
    /// range.
    pub fn select_fix(&mut self, index: usize) {
        if index < self.current_fixes().len() {
            self.fix_cursor = index;
        }
    }

    fn clamp(&mut self) {
        let nodes = self.nodes().len();
        if self.cursor >= nodes {
            self.cursor = nodes - 1;
        }
        let fixes = self.current_fixes().len();
        if fixes == 0 {
            self.fix_cursor = 0;
        } else if self.fix_cursor >= fixes {
            self.fix_cursor = fixes - 1;
        }
    }
}

/// Cap a platform area's status at `Partial` — a `Missing` platform component
/// is a non-blocking gap for rollup/glyph purposes; only the core
/// area ever surfaces `Missing`.
fn cap_platform(status: ComponentStatus) -> ComponentStatus {
    match status {
        ComponentStatus::Missing => ComponentStatus::Partial,
        other => other,
    }
}

/// The worst status in an iterator (`Missing` > `Partial` > `Ok`; `Ok` for an
/// empty iterator) — the same rollup fold the drive report uses.
fn worst(statuses: impl Iterator<Item = ComponentStatus>) -> ComponentStatus {
    statuses.fold(ComponentStatus::Ok, |acc, s| match (acc, s) {
        (ComponentStatus::Missing, _) | (_, ComponentStatus::Missing) => ComponentStatus::Missing,
        (ComponentStatus::Partial, _) | (_, ComponentStatus::Partial) => ComponentStatus::Partial,
        _ => ComponentStatus::Ok,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use frust_drive::doctor::Area;

    fn fix_runnable(display: &str, program: &str, args: &[&str]) -> FixCommand {
        FixCommand {
            display: display.to_string(),
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            auto_runnable: true,
            doc_link: None,
        }
    }

    fn fix_guidance(display: &str, doc: &str) -> FixCommand {
        FixCommand {
            display: display.to_string(),
            program: String::new(),
            args: Vec::new(),
            auto_runnable: false,
            doc_link: Some(doc.to_string()),
        }
    }

    fn component(name: &str, status: ComponentStatus, fixes: Vec<FixCommand>) -> Component {
        Component {
            name: name.to_string(),
            status,
            summary: format!("{name} summary"),
            fix_commands: fixes,
        }
    }

    /// A macOS-shaped report: green core, an Android area with a runnable
    /// cargo-ndk fix and a guidance-only JDK fix, a green iOS area, an
    /// always-green Desktop area.
    pub(crate) fn partial_report() -> DoctorReport {
        DoctorReport {
            areas: vec![
                Area {
                    name: CORE_AREA.to_string(),
                    components: vec![component("Rust toolchain", ComponentStatus::Ok, vec![])],
                },
                Area {
                    name: "Android".to_string(),
                    components: vec![
                        component("Android Rust targets", ComponentStatus::Ok, vec![]),
                        component(
                            "cargo-ndk",
                            ComponentStatus::Missing,
                            vec![fix_runnable(
                                "cargo install cargo-ndk",
                                "cargo",
                                &["install", "cargo-ndk"],
                            )],
                        ),
                        component(
                            "JDK",
                            ComponentStatus::Missing,
                            vec![fix_guidance(
                                "Install a JDK 17+",
                                "https://developer.android.com/studio",
                            )],
                        ),
                    ],
                },
                Area {
                    name: "iOS".to_string(),
                    components: vec![component("Xcode", ComponentStatus::Ok, vec![])],
                },
                Area {
                    name: "Desktop".to_string(),
                    components: vec![component("Desktop preview", ComponentStatus::Ok, vec![])],
                },
            ],
        }
    }

    fn missing_core_report() -> DoctorReport {
        let mut r = partial_report();
        r.areas[0].components[0].status = ComponentStatus::Missing;
        r.areas[0].components[0].fix_commands =
            vec![fix_guidance("Install Rust via rustup", "https://rustup.rs")];
        r
    }

    #[test]
    fn nodes_project_core_platforms_and_rollup() {
        let w = BootstrapWizard::from_report(partial_report());
        let nodes = w.nodes();
        // core "Rust toolchain" + Platforms header + Android/iOS/Desktop + Rollup
        assert_eq!(nodes.len(), 1 + 1 + 3 + 1);
        assert!(matches!(nodes[0], BootstrapNode::Core { .. }));
        assert!(matches!(nodes[1], BootstrapNode::PlatformsHeader));
        assert!(matches!(nodes[2], BootstrapNode::Platform { .. }));
        assert!(matches!(nodes[5], BootstrapNode::Rollup));
    }

    #[test]
    fn collapsing_platforms_hides_the_platform_leaves() {
        let mut w = BootstrapWizard::from_report(partial_report());
        assert_eq!(w.nodes().len(), 6);
        // Select and collapse the header.
        w.cursor = 1;
        w.toggle_expand();
        // core + header + rollup only.
        assert_eq!(w.nodes().len(), 3);
        assert!(matches!(w.nodes()[1], BootstrapNode::PlatformsHeader));
        assert!(matches!(w.nodes()[2], BootstrapNode::Rollup));
    }

    #[test]
    fn rollup_is_partial_on_a_platform_gap_and_never_blocks() {
        let w = BootstrapWizard::from_report(partial_report());
        assert_eq!(w.rollup(), ComponentStatus::Partial);
        assert!(!w.core_gates(), "a platform gap never gates handback");
    }

    #[test]
    fn a_missing_core_gates_handback() {
        let w = BootstrapWizard::from_report(missing_core_report());
        assert_eq!(w.rollup(), ComponentStatus::Missing);
        assert!(w.core_gates());
    }

    #[test]
    fn platforms_header_status_rolls_up_the_platform_areas() {
        let w = BootstrapWizard::from_report(partial_report());
        assert_eq!(
            w.node_status(BootstrapNode::PlatformsHeader),
            ComponentStatus::Partial
        );
    }

    #[test]
    fn selecting_the_android_area_lists_its_components_and_fixes() {
        let mut w = BootstrapWizard::from_report(partial_report());
        // Android is the first platform leaf (cursor 2).
        w.cursor = 2;
        assert_eq!(w.detail_components().len(), 3);
        // Two fixes: cargo-ndk (runnable) + JDK (guidance).
        let fixes = w.current_fixes();
        assert_eq!(fixes.len(), 2);
        assert!(fixes[0].1.auto_runnable);
        assert!(!fixes[1].1.auto_runnable);
    }

    #[test]
    fn runnable_selected_fix_gates_on_auto_runnable() {
        let mut w = BootstrapWizard::from_report(partial_report());
        w.cursor = 2; // Android
        // Default fix cursor 0 → the runnable cargo-ndk fix.
        assert!(w.runnable_selected_fix().is_some());
        assert_eq!(w.runnable_selected_fix().unwrap().program, "cargo");
        // Move to the guidance-only JDK fix → no runnable fix.
        assert!(w.nav_fix(1));
        assert!(w.selected_fix().is_some());
        assert!(w.runnable_selected_fix().is_none());
    }

    #[test]
    fn nav_clamps_and_resets_the_fix_cursor_on_a_step_change() {
        let mut w = BootstrapWizard::from_report(partial_report());
        w.cursor = 2; // Android
        w.nav_fix(1); // fix_cursor = 1
        assert_eq!(w.fix_cursor, 1);
        // Moving the step cursor resets the fix cursor.
        assert!(w.nav(1)); // -> iOS
        assert_eq!(w.fix_cursor, 0);
        // Clamped at both ends.
        w.cursor = 0;
        assert!(!w.nav(-1));
    }

    #[test]
    fn a_header_only_node_has_no_fixes() {
        let mut w = BootstrapWizard::from_report(partial_report());
        w.cursor = 1; // Platforms header
        assert!(w.current_fixes().is_empty());
        assert!(w.selected_fix().is_none());
    }

    #[test]
    fn set_report_reclamps_cursors() {
        let mut w = BootstrapWizard::from_report(partial_report());
        w.cursor = 5; // Rollup
        w.set_report(missing_core_report());
        // Still in range for the (same-shaped) report.
        assert!(w.cursor < w.nodes().len());
    }
}
