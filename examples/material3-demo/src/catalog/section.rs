//! The five gallery sections, one per navigation-bar destination.

use frust::IconSource;
use frust_material::icons;

/// Width at or above which a section shows its list and the selected
/// playground side by side (logical px) — the reference's own
/// `kM3EDemoSplitBreakpoint`.
pub const SPLIT_BREAKPOINT_PX: f64 = 900.0;

/// A gallery section: one navigation-bar destination and the catalog entries
/// filed under it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DemoSection {
    /// Actions — Do.
    Do,
    /// Selection — Pick.
    Pick,
    /// Containment — View.
    View,
    /// Navigation — Nav.
    Nav,
    /// Feedback / input — Find.
    Find,
}

impl DemoSection {
    /// Every section, in navigation-bar order. The index into this array is
    /// the navigation bar's own selected index.
    pub const ALL: [DemoSection; 5] = [
        DemoSection::Do,
        DemoSection::Pick,
        DemoSection::View,
        DemoSection::Nav,
        DemoSection::Find,
    ];

    /// Short navigation-bar label.
    pub fn nav_label(self) -> &'static str {
        match self {
            DemoSection::Do => "Do",
            DemoSection::Pick => "Pick",
            DemoSection::View => "View",
            DemoSection::Nav => "Nav",
            DemoSection::Find => "Find",
        }
    }

    /// Navigation-bar icon.
    pub fn nav_icon(self) -> IconSource {
        match self {
            DemoSection::Do => icons::ADD,
            DemoSection::Pick => icons::CHECK,
            DemoSection::View => icons::CALENDAR_TODAY,
            DemoSection::Nav => icons::MENU,
            DemoSection::Find => icons::SEARCH,
        }
    }

    /// The section's route, a child of the gallery shell route.
    pub fn path(self) -> &'static str {
        match self {
            DemoSection::Do => "/do",
            DemoSection::Pick => "/pick",
            DemoSection::View => "/view",
            DemoSection::Nav => "/nav",
            DemoSection::Find => "/find",
        }
    }

    /// The batch a not-yet-ported section's playgrounds are landing in — the
    /// number the coming-soon placeholder quotes.
    pub fn batch_number(self) -> u32 {
        match self {
            DemoSection::Do => 1,
            DemoSection::Pick => 2,
            DemoSection::View => 3,
            DemoSection::Nav => 4,
            DemoSection::Find => 5,
        }
    }

    /// This section's index in [`DemoSection::ALL`].
    pub fn index(self) -> usize {
        DemoSection::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0)
    }

    /// The section owning `path`, or `None` for anything outside the shell.
    pub fn from_path(path: &str) -> Option<DemoSection> {
        DemoSection::ALL
            .into_iter()
            .find(|section| section.path() == path)
    }
}

#[cfg(test)]
mod tests {
    use super::DemoSection;

    #[test]
    fn every_section_round_trips_through_its_path() {
        for (index, section) in DemoSection::ALL.into_iter().enumerate() {
            assert_eq!(DemoSection::from_path(section.path()), Some(section));
            assert_eq!(section.index(), index);
        }
    }

    #[test]
    fn a_path_outside_the_shell_names_no_section() {
        assert_eq!(DemoSection::from_path("/theme"), None);
        assert_eq!(DemoSection::from_path("/playground/buttons"), None);
    }
}
