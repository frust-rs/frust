//! The devices-panel row model and the run-config modal state machine:
//! device(s) + `BuildInfo` (mode / flavor / defines) → one supervised
//! session per selected target.
//!
//! Everything here is plain data + pure transitions — no threads, no process,
//! no terminal — so the modal's focus/selection/launch-spec logic is
//! unit-tested without a TTY. `crate::ui` renders it; `crate::runner` enacts
//! the launch [`Effect`](super::update::Effect).

use std::collections::HashMap;
use std::path::PathBuf;

use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::devices::Device;

use crate::supervise::{DeviceTarget, SessionSpec};

/// One row in the devices sidebar: a discovered device plus its panel
/// multi-select flag (`Space` toggles it; opening the run-config primes the
/// modal's checklist from these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// The discovered device.
    pub device: Device,
    /// Whether this row is selected in the panel's multi-select.
    pub selected: bool,
}

impl DeviceRow {
    /// A freshly-discovered, unselected row.
    pub fn new(device: Device) -> Self {
        Self {
            device,
            selected: false,
        }
    }
}

/// A launch target in the run-config modal: the desktop preview or a device,
/// each independently checkable. The modal always offers desktop plus every
/// discovered device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTarget {
    /// The short label shown in the checklist (`desktop`, or the device name).
    pub label: String,
    /// The concrete launch target.
    pub target: DeviceTarget,
    /// Whether this target is checked (will be launched).
    pub selected: bool,
}

/// Which control in the run-config modal has keyboard focus. Ordered
/// top-to-bottom: the target checkboxes, then mode, flavor, defines, the
/// watch checkbox, and the launch button — the order [`RunConfig::focus_next`]/[`focus_prev`] walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFocus {
    /// A target checkbox row (0-based index into [`RunConfig::targets`]).
    Target(usize),
    /// The build-mode selector (`←`/`→` cycles debug/profile/release).
    Mode,
    /// The flavor text field.
    Flavor,
    /// The defines text field (`KEY=VALUE` pairs, space-separated).
    Defines,
    /// The "Watch src/ and restart on change (desktop only)" checkbox
    /// (`Space` toggles it).
    Watch,
    /// The launch button.
    Launch,
}

/// The run-config modal state: the target checklist plus the
/// `BuildInfo` funnel, with a single focused control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// The project the launched sessions build/run (the active project when
    /// the modal was opened) — every spec groups under it.
    pub project_root: PathBuf,
    /// The launch checklist: desktop first, then each discovered device.
    pub targets: Vec<RunTarget>,
    /// The build mode.
    pub mode: BuildMode,
    /// The product flavor (empty = none).
    pub flavor: String,
    /// The `--define`s as typed (`KEY=VALUE` pairs, space-separated).
    pub defines: String,
    /// Whether the launched **desktop** session gets "Watch: restart on
    /// save" turned on as soon as it registers (the runner's
    /// `Message::EnableWatch`). Device targets ignore it — the watch loop
    /// has no device-side story. Off by default.
    pub watch: bool,
    /// Which control has focus.
    pub focus: RunFocus,
}

impl RunConfig {
    /// Build a modal for `project_root`, priming the checklist: desktop
    /// (unchecked) followed by each device — a device checked iff its panel
    /// row was selected. Focus starts on the first target row.
    pub fn new(project_root: PathBuf, devices: &[DeviceRow]) -> Self {
        let mut targets = vec![RunTarget {
            label: "desktop".to_string(),
            target: DeviceTarget::Desktop,
            selected: false,
        }];
        for row in devices {
            targets.push(RunTarget {
                label: row.device.name.clone(),
                target: DeviceTarget::Device(row.device.clone()),
                selected: row.selected,
            });
        }
        // If nothing was pre-selected, default to desktop so a bare
        // `r`→`Enter` still launches something sensible.
        if !targets.iter().any(|t| t.selected) {
            targets[0].selected = true;
        }
        Self {
            project_root,
            targets,
            mode: BuildMode::Debug,
            flavor: String::new(),
            defines: String::new(),
            watch: false,
            focus: RunFocus::Target(0),
        }
    }

    /// The focus order as a flat list, top to bottom.
    fn focus_order(&self) -> Vec<RunFocus> {
        let mut order: Vec<RunFocus> = (0..self.targets.len()).map(RunFocus::Target).collect();
        order.push(RunFocus::Mode);
        order.push(RunFocus::Flavor);
        order.push(RunFocus::Defines);
        order.push(RunFocus::Watch);
        order.push(RunFocus::Launch);
        order
    }

    /// Move focus to the next control (wrapping).
    pub fn focus_next(&mut self) {
        self.step_focus(1);
    }

    /// Move focus to the previous control (wrapping).
    pub fn focus_prev(&mut self) {
        self.step_focus(-1);
    }

    fn step_focus(&mut self, delta: isize) {
        let order = self.focus_order();
        let cur = order.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(order.len() as isize) as usize;
        self.focus = order[next];
    }

    /// Toggle the focused checkbox: a target row's selection, or the watch
    /// checkbox (no-op on any other control).
    pub fn toggle_focused_target(&mut self) {
        match self.focus {
            RunFocus::Target(i) => {
                if let Some(t) = self.targets.get_mut(i) {
                    t.selected = !t.selected;
                }
            }
            RunFocus::Watch => self.watch = !self.watch,
            RunFocus::Mode | RunFocus::Flavor | RunFocus::Defines | RunFocus::Launch => {}
        }
    }

    /// Toggle the watch checkbox, focusing it (mouse click parity).
    pub fn toggle_watch(&mut self) {
        self.watch = !self.watch;
        self.focus = RunFocus::Watch;
    }

    /// Toggle a target by index (mouse click parity).
    pub fn toggle_target(&mut self, index: usize) {
        if let Some(t) = self.targets.get_mut(index) {
            t.selected = !t.selected;
            self.focus = RunFocus::Target(index);
        }
    }

    /// Cycle the build mode by `delta` (`+1`/`-1`), wrapping debug → profile →
    /// release → debug.
    pub fn cycle_mode(&mut self, delta: isize) {
        const MODES: [BuildMode; 3] = [BuildMode::Debug, BuildMode::Profile, BuildMode::Release];
        let cur = MODES.iter().position(|m| *m == self.mode).unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(MODES.len() as isize) as usize;
        self.mode = MODES[next];
    }

    /// Feed a character into the focused text field (flavor/defines); no-op on
    /// any other control.
    pub fn input_char(&mut self, c: char) {
        match self.focus {
            RunFocus::Flavor => self.flavor.push(c),
            RunFocus::Defines => self.defines.push(c),
            _ => {}
        }
    }

    /// Delete the last character of the focused text field; no-op elsewhere.
    pub fn backspace(&mut self) {
        match self.focus {
            RunFocus::Flavor => {
                self.flavor.pop();
            }
            RunFocus::Defines => {
                self.defines.pop();
            }
            _ => {}
        }
    }

    /// Whether at least one target is checked (a launch precondition).
    pub fn any_selected(&self) -> bool {
        self.targets.iter().any(|t| t.selected)
    }

    /// The `BuildInfo` the checked build fields resolve to. `defines` are
    /// parsed as space-separated `KEY=VALUE` pairs; a malformed token (no `=`
    /// or an empty key) is skipped rather than failing the launch.
    pub fn build_info(&self) -> BuildInfo {
        let mut defines = HashMap::new();
        for token in self.defines.split_whitespace() {
            if let Some((k, v)) = token.split_once('=')
                && !k.is_empty()
            {
                defines.insert(k.to_string(), v.to_string());
            }
        }
        let flavor = {
            let f = self.flavor.trim();
            (!f.is_empty()).then(|| f.to_string())
        };
        BuildInfo {
            mode: self.mode,
            flavor,
            defines,
            build_name: None,
            build_number: None,
        }
    }

    /// One [`SessionSpec`] per checked target — what the runner hands the
    /// supervisor to launch N concurrent sessions grouped under this project.
    pub fn launch_specs(&self) -> Vec<SessionSpec> {
        let build = self.build_info();
        self.targets
            .iter()
            .filter(|t| t.selected)
            .map(|t| SessionSpec {
                project_root: self.project_root.clone(),
                target: t.target.clone(),
                build: build.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::devices::{Kind, Platform};
    use std::path::{Path, PathBuf};

    fn device(id: &str, name: &str, platform: Platform, kind: Kind) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            platform,
            kind,
            os_version: None,
            connection_state: None,
        }
    }

    fn rows() -> Vec<DeviceRow> {
        vec![
            DeviceRow {
                device: device(
                    "emulator-5554",
                    "Pixel 7",
                    Platform::Android,
                    Kind::Emulator,
                ),
                selected: true,
            },
            DeviceRow {
                device: device("AAAA", "iPhone 15", Platform::Ios, Kind::Simulator),
                selected: false,
            },
        ]
    }

    fn modal() -> RunConfig {
        RunConfig::new(PathBuf::from("/tmp/huddle"), &rows())
    }

    #[test]
    fn new_primes_desktop_plus_devices_and_carries_panel_selection() {
        let m = modal();
        // desktop + 2 devices.
        assert_eq!(m.targets.len(), 3);
        assert_eq!(m.targets[0].target, DeviceTarget::Desktop);
        // The pre-selected Pixel 7 comes through checked; desktop and the
        // iPhone are unchecked (a panel selection existed, so no desktop
        // default is forced).
        assert!(!m.targets[0].selected);
        assert!(m.targets[1].selected);
        assert!(!m.targets[2].selected);
        assert_eq!(m.focus, RunFocus::Target(0));
    }

    #[test]
    fn new_with_no_panel_selection_defaults_to_desktop() {
        let mut r = rows();
        r[0].selected = false;
        let m = RunConfig::new(PathBuf::from("/tmp/a"), &r);
        assert!(
            m.targets[0].selected,
            "desktop defaults on when nothing preselected"
        );
    }

    #[test]
    fn focus_walks_targets_then_fields_then_launch_and_wraps() {
        let mut m = modal();
        let order = [
            RunFocus::Target(0),
            RunFocus::Target(1),
            RunFocus::Target(2),
            RunFocus::Mode,
            RunFocus::Flavor,
            RunFocus::Defines,
            RunFocus::Watch,
            RunFocus::Launch,
        ];
        for expected in order.iter().skip(1) {
            m.focus_next();
            assert_eq!(m.focus, *expected);
        }
        // Wrap forward past Launch back to the first target.
        m.focus_next();
        assert_eq!(m.focus, RunFocus::Target(0));
        // And wrap backward.
        m.focus_prev();
        assert_eq!(m.focus, RunFocus::Launch);
    }

    #[test]
    fn space_toggles_only_the_focused_target() {
        let mut m = modal();
        m.focus = RunFocus::Target(0);
        m.toggle_focused_target();
        assert!(m.targets[0].selected);
        m.focus = RunFocus::Mode;
        // No-op off a target row.
        m.toggle_focused_target();
        assert!(m.targets[0].selected);
    }

    #[test]
    fn cycle_mode_wraps_debug_profile_release() {
        let mut m = modal();
        assert_eq!(m.mode, BuildMode::Debug);
        m.cycle_mode(1);
        assert_eq!(m.mode, BuildMode::Profile);
        m.cycle_mode(1);
        assert_eq!(m.mode, BuildMode::Release);
        m.cycle_mode(1);
        assert_eq!(m.mode, BuildMode::Debug);
        m.cycle_mode(-1);
        assert_eq!(m.mode, BuildMode::Release);
    }

    #[test]
    fn text_input_only_edits_the_focused_field() {
        let mut m = modal();
        m.focus = RunFocus::Flavor;
        for c in "paid".chars() {
            m.input_char(c);
        }
        m.focus = RunFocus::Defines;
        for c in "A=1 B=2".chars() {
            m.input_char(c);
        }
        m.focus = RunFocus::Target(0);
        m.input_char('z'); // dropped
        assert_eq!(m.flavor, "paid");
        assert_eq!(m.defines, "A=1 B=2");
        m.focus = RunFocus::Defines;
        m.backspace();
        assert_eq!(m.defines, "A=1 B=");
    }

    #[test]
    fn build_info_parses_flavor_and_defines() {
        let mut m = modal();
        m.flavor = "  paid ".into();
        m.defines = "FRUST_TRACE=1 bad-token EMPTY= =NOKEY B=2".into();
        m.mode = BuildMode::Release;
        let info = m.build_info();
        assert_eq!(info.mode, BuildMode::Release);
        assert_eq!(info.flavor.as_deref(), Some("paid"));
        assert_eq!(info.defines.get("FRUST_TRACE"), Some(&"1".to_string()));
        assert_eq!(info.defines.get("EMPTY"), Some(&String::new()));
        assert_eq!(info.defines.get("B"), Some(&"2".to_string()));
        // `bad-token` (no `=`) and `=NOKEY` (empty key) are skipped.
        assert!(!info.defines.contains_key("bad-token"));
        assert_eq!(info.defines.len(), 3);
    }

    #[test]
    fn launch_specs_is_one_per_checked_target() {
        let mut m = modal();
        // Pre-checked: Pixel 7 only. Also check desktop.
        m.targets[0].selected = true;
        let specs = m.launch_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].target, DeviceTarget::Desktop);
        assert!(matches!(specs[1].target, DeviceTarget::Device(ref d) if d.name == "Pixel 7"));
        // Every spec shares the project root and build funnel.
        assert!(
            specs
                .iter()
                .all(|s| s.project_root == Path::new("/tmp/huddle"))
        );
    }

    #[test]
    fn toggle_target_by_index_moves_focus_there() {
        let mut m = modal();
        m.toggle_target(2);
        assert!(m.targets[2].selected);
        assert_eq!(m.focus, RunFocus::Target(2));
    }

    #[test]
    fn watch_starts_off_and_space_or_a_click_toggles_it() {
        let mut m = modal();
        assert!(!m.watch, "watch is opt-in");
        m.focus = RunFocus::Watch;
        m.toggle_focused_target();
        assert!(m.watch);
        // The checkbox never touches a target's selection.
        assert!(!m.targets[0].selected);
        m.focus = RunFocus::Target(0);
        m.toggle_watch();
        assert!(!m.watch);
        assert_eq!(m.focus, RunFocus::Watch, "a click focuses the checkbox");
    }
}
