//! exportOptions plist generation + `-exportArchive` argument assembly for
//! `frust build ipa`. The plist is a tiny fixed-schema XML string —
//! `method` (required) plus `teamID` (when known) — hand-rolled so
//! `ios_build` needs no plist/serde dependency.

use super::xcodebuild::ARCHIVE_PATH;

/// Where the generated exportOptions plist is written, relative to root.
pub const EXPORT_OPTIONS_PATH: &str = "build/ios/ExportOptions.plist";

/// Where `-exportArchive` writes the `.ipa`, relative to root.
pub const EXPORT_PATH: &str = "build/ios/ipa";

/// Renders the exportOptions plist for `method` (the `--export-method` value),
/// including `teamID` when a team was resolved.
pub fn export_options_plist(method: &str, team: Option<&str>) -> String {
    let mut dict = format!("  <key>method</key><string>{method}</string>\n");
    if let Some(team) = team {
        dict.push_str(&format!("  <key>teamID</key><string>{team}</string>\n"));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n{dict}</dict></plist>\n"
    )
}

/// The full argv (after `xcrun`) for `-exportArchive`.
pub fn export_argv() -> Vec<String> {
    vec![
        "xcodebuild".to_string(),
        "-exportArchive".to_string(),
        "-archivePath".to_string(),
        ARCHIVE_PATH.to_string(),
        "-exportOptionsPlist".to_string(),
        EXPORT_OPTIONS_PATH.to_string(),
        "-exportPath".to_string(),
        EXPORT_PATH.to_string(),
        "-allowProvisioningUpdates".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_with_team_matches_golden() {
        let plist = export_options_plist("app-store-connect", Some("ABCDE12345"));
        let expected = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\"><dict>\n",
            "  <key>method</key><string>app-store-connect</string>\n",
            "  <key>teamID</key><string>ABCDE12345</string>\n",
            "</dict></plist>\n",
        );
        assert_eq!(plist, expected);
    }

    #[test]
    fn plist_without_team_omits_teamid_key() {
        let plist = export_options_plist("release-testing", None);
        assert!(plist.contains("<key>method</key><string>release-testing</string>"));
        assert!(!plist.contains("teamID"));
    }

    #[test]
    fn export_argv_is_the_expected_invocation() {
        assert_eq!(
            export_argv(),
            [
                "xcodebuild",
                "-exportArchive",
                "-archivePath",
                "build/ios/archive/Runner.xcarchive",
                "-exportOptionsPlist",
                "build/ios/ExportOptions.plist",
                "-exportPath",
                "build/ios/ipa",
                "-allowProvisioningUpdates",
            ]
        );
    }
}
