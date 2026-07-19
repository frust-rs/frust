//! Gradle task-name computation and `-P` property assembly (spec §12.5,
//! PLAN.md decision 2): `assemble<Flavor><Mode>`/`bundle<Flavor><Mode>` task
//! selection, plus the `-Pfrust.*` properties template 62's
//! `build.gradle.kts.tmpl` consumes.

use std::collections::HashMap;

use crate::android_build::AndroidArtifact;
use crate::build_info::BuildMode;

/// Gradle ABI names for every supported target platform, in the order
/// `commands::build::TARGET_PLATFORMS` maps `--target-platform` values —
/// what `build appbundle` always requests (an `.aab` packages every ABI).
pub const ALL_ABIS: &[&str] = &["arm64-v8a", "armeabi-v7a", "x86_64"];

/// Computes the Gradle task name for `target`/`mode`/`flavor` (spec §12.5):
/// `assemble<Flavor><Mode>` for an APK, `bundle<Flavor><Mode>` for an App
/// Bundle. `flavor` is capitalized (first char upper, rest as given) and
/// omitted entirely when `None`.
pub fn task_name(target: &AndroidArtifact, mode: BuildMode, flavor: Option<&str>) -> String {
    let verb = match target {
        AndroidArtifact::Apk { .. } => "assemble",
        AndroidArtifact::Appbundle => "bundle",
    };
    let flavor_part = flavor.map(capitalize).unwrap_or_default();
    format!("{verb}{flavor_part}{}", mode.gradle_infix())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Assembles the `-P` properties template 62's `build.gradle.kts.tmpl`
/// consumes: `-Pfrust.targetPlatforms=<abi,csv>` (always),
/// `-Pfrust.splitPerAbi=true|false` (APK only), and
/// `-Pfrust.defines=<base64("K=V;K=V")>` when `defines` is non-empty.
pub fn gradle_properties(
    target: &AndroidArtifact,
    defines: &HashMap<String, String>,
) -> Vec<String> {
    let mut props = Vec::new();

    match target {
        AndroidArtifact::Apk {
            split_per_abi,
            abis,
        } => {
            props.push(format!("-Pfrust.targetPlatforms={}", abis.join(",")));
            props.push(format!("-Pfrust.splitPerAbi={split_per_abi}"));
        }
        AndroidArtifact::Appbundle => {
            props.push(format!("-Pfrust.targetPlatforms={}", ALL_ABIS.join(",")));
        }
    }

    if !defines.is_empty() {
        props.push(format!("-Pfrust.defines={}", encode_defines(defines)));
    }

    props
}

/// Encodes `defines` as `base64("K=V;K=V")`, sorted by key for a
/// deterministic, testable output (a `HashMap`'s natural iteration order is
/// not stable) — the exact value template 62's `build.gradle.kts.tmpl`
/// decodes with `Base64.getDecoder()`.
fn encode_defines(defines: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = defines.keys().collect();
    keys.sort();
    let joined = keys
        .iter()
        .map(|k| format!("{k}={}", defines[*k]))
        .collect::<Vec<_>>()
        .join(";");
    base64_encode(joined.as_bytes())
}

const BASE64_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A hand-rolled standard (RFC 4648, padded) base64 encoder — matches
/// `java.util.Base64.getDecoder()`'s expected alphabet on the Gradle side
/// (CODE_STANDARDS: hand-roll at the platform-template boundary rather than
/// add a dependency for a single call site).
fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(BASE64_ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(BASE64_ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[((n >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apk(abis: &[&str], split_per_abi: bool) -> AndroidArtifact {
        AndroidArtifact::Apk {
            split_per_abi,
            abis: abis.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn task_name_table() {
        let cases: &[(&str, AndroidArtifact, BuildMode, Option<&str>, &str)] = &[
            (
                "apk debug no flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Debug,
                None,
                "assembleDebug",
            ),
            (
                "apk release no flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Release,
                None,
                "assembleRelease",
            ),
            (
                "apk profile no flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Profile,
                None,
                "assembleProfile",
            ),
            (
                "apk release with flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Release,
                Some("paid"),
                "assemblePaidRelease",
            ),
            (
                "apk debug with already-capitalized flavor",
                apk(&["arm64-v8a"], false),
                BuildMode::Debug,
                Some("Paid"),
                "assemblePaidDebug",
            ),
            (
                "appbundle release no flavor",
                AndroidArtifact::Appbundle,
                BuildMode::Release,
                None,
                "bundleRelease",
            ),
            (
                "appbundle release with flavor",
                AndroidArtifact::Appbundle,
                BuildMode::Release,
                Some("paid"),
                "bundlePaidRelease",
            ),
            (
                "appbundle profile with flavor",
                AndroidArtifact::Appbundle,
                BuildMode::Profile,
                Some("free"),
                "bundleFreeProfile",
            ),
        ];

        for (label, target, mode, flavor, expected) in cases {
            assert_eq!(
                &task_name(target, *mode, *flavor),
                expected,
                "case: {label}"
            );
        }
    }

    #[test]
    fn gradle_properties_apk_includes_target_platforms_and_split_per_abi() {
        let target = apk(&["arm64-v8a", "x86_64"], true);
        let props = gradle_properties(&target, &HashMap::new());
        assert_eq!(
            props,
            vec![
                "-Pfrust.targetPlatforms=arm64-v8a,x86_64".to_string(),
                "-Pfrust.splitPerAbi=true".to_string(),
            ]
        );
    }

    #[test]
    fn gradle_properties_appbundle_always_requests_all_abis_and_omits_split_flag() {
        let props = gradle_properties(&AndroidArtifact::Appbundle, &HashMap::new());
        assert_eq!(
            props,
            vec!["-Pfrust.targetPlatforms=arm64-v8a,armeabi-v7a,x86_64".to_string()]
        );
    }

    #[test]
    fn gradle_properties_omits_defines_prop_when_empty() {
        let target = apk(&["arm64-v8a"], false);
        let props = gradle_properties(&target, &HashMap::new());
        assert!(props.iter().all(|p| !p.starts_with("-Pfrust.defines=")));
    }

    #[test]
    fn gradle_properties_appends_base64_defines_when_present() {
        let target = apk(&["arm64-v8a"], false);
        let mut defines = HashMap::new();
        defines.insert("A".to_string(), "1".to_string());
        defines.insert("B".to_string(), "2".to_string());
        let props = gradle_properties(&target, &defines);
        let defines_prop = props
            .iter()
            .find(|p| p.starts_with("-Pfrust.defines="))
            .expect("defines prop present");
        // "A=1;B=2" (sorted by key) base64-encoded — verified by hand
        // against the standard RFC 4648 alphabet.
        assert_eq!(defines_prop, "-Pfrust.defines=QT0xO0I9Mg==");
    }

    #[test]
    fn base64_encode_known_value() {
        // "A=B" -> 0x41 0x3D 0x42 -> 6-bit groups 010000 010011 110101
        // 000010 -> Q T 1 C
        assert_eq!(base64_encode(b"A=B"), "QT1C");
    }

    #[test]
    fn base64_encode_round_trips_via_reference_decoder() {
        let cases = ["", "A", "AB", "ABC", "API_URL=https://example.com;DEBUG=1"];
        for case in cases {
            let encoded = base64_encode(case.as_bytes());
            let decoded = base64_decode(&encoded);
            assert_eq!(decoded, case.as_bytes(), "case: {case:?}");
        }
    }

    /// Minimal standard-alphabet base64 decoder, used only by
    /// [`base64_encode_round_trips_via_reference_decoder`] to independently
    /// verify [`base64_encode`] — not used by production code (Gradle/Kotlin
    /// owns decoding on the consuming side).
    fn base64_decode(input: &str) -> Vec<u8> {
        fn value(c: u8) -> Option<u8> {
            BASE64_ALPHABET
                .iter()
                .position(|&b| b == c)
                .map(|p| p as u8)
        }
        let trimmed = input.trim_end_matches('=');
        let mut out = Vec::new();
        let mut buf = 0u32;
        let mut bits = 0u32;
        for c in trimmed.bytes() {
            let v = value(c).expect("valid base64 char");
            buf = (buf << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
            }
        }
        out
    }
}
